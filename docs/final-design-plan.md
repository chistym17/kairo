# Kairo Runtime Architecture — Final Design Plan

Status: **Accepted, pending implementation validation.** This document is the converged output of the design review that followed `annotaa/design-report.md`. It supersedes that report's §7–§10 (recommended architecture, invariants, state model, invalid-action/recovery design) and its implementation-slice plan (§14–§15). Sections §1–§6, §11–§13, §16–§18 of `design-report.md` (the factual architecture map of Kairo and Pi, provider considerations, and code-to-remove list) remain valid background and are not restated here.

No runtime code has been written against this plan yet. The next step after this document is Slice 1 (§8), followed by benchmark evidence — not further design iteration.

---

## 1. Guiding principle

> **Minimize stored control state. Store only facts that cannot be recomputed. Everything else — policies, permissions, prompts, display phases, completion eligibility, and recovery affordances — is derived from those facts.**

Every other decision in this document is a consequence of this one rule. It explains why `ToolScope`, `DisplayPhase`, and `CompletionEligibility` are all pure functions rather than stored fields, why authorization is authoritative state rather than a stateless check, and why context/prompt construction is a derivation rather than hand-assembled text.

## 2. The shift this plan represents

The original investigation (`design-report.md`) approached Kairo's problems by asking phase-centric questions: *when do we enter Act? When do we leave Verify? Should this tool exist in this phase?* Each answer became a new counter, a new transition rule, or a new special case, and they didn't compose — which is the root cause identified in `design-report.md` §3.

This plan asks state-centric questions instead: *what facts are currently true? Given those facts, what policy is derived? Given that policy, how should this action be evaluated? Given that evaluation, how should the state change?* `Explore/Act/Verify/Finish` survive only as a **derived display label**, never as something the enforcement path branches on.

## 3. Runtime architecture: four layers, one loop

```
┌────────────────────────────────────────────────┐
│ Layer 1 — Authoritative State                   │
│                                                  │
│   Evidence        (seen files, seen searches)   │
│   Edit             (current edit target, if any)│
│   Recovery         (active recovery grant/target)│
│   Verification     (attempts, last result)      │
│   Authorization    (approval granted/denied/pending)│
│   Progress         (stall count, stall history) │
└───────────────────────┬──────────────────────────┘
                         │  (pure functions)
                         ▼
┌────────────────────────────────────────────────┐
│ Layer 2 — Derived Policy                        │
│                                                  │
│   ActionPolicy         (which tools, what scope)│
│   ToolScope             (per-tool access bound) │
│   Context Snapshot      (model-facing prompt)   │
│   CompletionEligibility (can this run end, how) │
│   DisplayPhase          (Explore/Act/Verify/Finish, UI only) │
└───────────────────────┬──────────────────────────┘
                         │  (pure functions, given an incoming action)
                         ▼
┌────────────────────────────────────────────────┐
│ Layer 3 — Action Evaluation                     │
│                                                  │
│   Validate    (name, arguments, scope)          │
│   Authorize   (approval-gated effects)          │
│   Execute     (dispatch — workspace side effects)│
│   Classify    (structured outcome judgment)     │
└───────────────────────┬──────────────────────────┘
                         │
                         ▼
┌────────────────────────────────────────────────┐
│ Layer 4 — Runtime Decision                      │
│                                                  │
│   Continue │ Recover │ Complete │ Terminate      │
└───────────────────────┬──────────────────────────┘
                         │
                         ▼
              mutate Authoritative State
                         │
                         └──────────────▶ next turn (back to Layer 1)
```

**Governing constraint:** *only Layer 4 may mutate Layer 1.* Layer 2 is entirely pure functions over the current state. Layer 3 is **not** entirely pure — it contains one necessary impure step (`Execute`, real workspace dispatch) sandwiched between two pure steps (`Validate`/`Authorize` before it, `Classify` after it) — but none of Layer 3's steps, pure or not, may themselves write to Layer 1; that is reserved for Layer 4 alone (see §6 for the exact function split that makes this enforceable rather than a convention). Without the closing arrow back into Layer 1, this is a request pipeline; with it, it is a runtime. Every implementation of this design must preserve that closed loop explicitly — it is not optional plumbing, it is the mechanism that replaces today's four independent counters with one authoritative record of what happened.

**Workspace vs. runtime control state.** `Execute` (Layer 3) may have real external side effects — editing files, running commands, creating processes. It does **not** directly mutate the runtime's authoritative control state. Tool outputs are first classified into structured outcomes by `Classify`, and only `Layer 4` may commit those outcomes into Layer 1:

```
Workspace (external, ground truth)      Runtime Control State (Layer 1)
─────────────────────────────────       ────────────────────────────────
Files, processes, git, terminal    ≠    Evidence, Recovery, Verification,
                                          Authorization, Progress
```

This means tool implementations (`apply_patch`, `read_file`, `run_command`) never need to know anything about phases, scopes, or recovery — they just produce a result. The runtime alone is responsible for interpreting that result into state.

## 4. Layer 1 — Authoritative state (detail)

| Field | Replaces (from current Kairo) |
|---|---|
| `Evidence` — seen files, seen searches, key-facts summary | `ExplorationTracker`, `ProgressState.evidence_keys` |
| `Edit` — current edit target, if any | (implicit today — not currently tracked as a first-class field) |
| `Recovery` — `{ reason, target, scope, budget }` for any active bounded exception | `PatchRecoveryState` + `RunState.patch_fail_recovery` (merged into one) |
| `Verification` — `{ attempts_for_target, last_result }` | `CompletionTracker`'s verify-related fields |
| `Authorization` — approval granted / denied / pending, session-level permissions | `AuthorizationState` (already exists; now explicitly part of the same state model instead of a parallel system) |
| `Progress` — `consecutive_stalls: u32`, `stall_history: [ActionFingerprint; 3-4]` (ring buffer) | `RunState.no_progress`, `ProgressState.consecutive_no_progress`, `RunState.consecutive_denials`, `RunState.consecutive_invalid_actions` (all four merged into one) |

**Rule:** every mutable runtime concept belongs to exactly one place in Layer 1. If two fields can independently claim the same fact (e.g. a `VerificationState` and a separate `FinishedBecauseVerified` boolean; a `RecoveryState` and a separately-tracked `RemainingRecoveryReads`), that is a design error — one of them must become a derivation of the other.

## 5. Layer 2 — Derived policy (detail)

### 5.1 Schema, capability, and scope are three distinct concepts

```
Schema      — what the model knows exists (the tool list sent to the provider)
Capability  — whether the runtime currently permits using it at all
Scope       — what that tool may operate on, right now
```

**Design rule:** during normal execution, tools should not disappear from the schema merely because the runtime's state changed. Prefer capability or scope changes over schema removal. A tool may remain visible in the schema with `AccessLevel::Disabled` and produce a structured, uniform failure if called — this is categorically different from removing it from the schema, because the model never has to explain why a tool it has used before has vanished. This is the direct fix for the brief's central failure case ("`read_file` after `Act` removes it"): the schema cliff, not the restriction itself, was the cause of the thrash.

Note on scope: Slice 1 (§8) applies this only to `read_file`. `search_code`/`find_files`/shell remain schema-removed in Act for the first slice — this is a stated, temporary inconsistency, not a claim that the schema-cliff problem is fully solved after Slice 1. Extending `Disabled`-while-present treatment to those tools is later work (§8, Slice 1 hypothesis).

### 5.2 `ToolScope`

```rust
struct ToolScope {
    access: AccessLevel,
    constraints: ScopeConstraints,
    budget: ScopeBudget,
}

enum AccessLevel {
    Unrestricted,
    Bounded,
    Disabled,
}

struct ScopeConstraints {
    allowed_paths: Option<HashSet<PathBuf>>,
    allowed_units: Option<HashSet<ProjectUnit>>,
}

enum ScopeBudget {
    Unlimited,
    Uses(u32),
}
```

Composable, not enumerated per tool — e.g. `ReadSeenFilesOnly`, `PatchFailedTargetOnly`, `RunEditedUnitOnly` are not separate enum variants; they are all just different field combinations of the same struct:

```rust
// Act read policy
ToolScope { access: Bounded, constraints: ScopeConstraints { allowed_paths: Some(seen_files), .. }, budget: Unlimited }

// Patch recovery
ToolScope { access: Bounded, constraints: ScopeConstraints { allowed_paths: Some({failed_path}), .. }, budget: Uses(1) }

// Verification shell
ToolScope { access: Bounded, constraints: ScopeConstraints { allowed_units: Some({edited_unit}), .. }, budget: Uses(1) }
```

`ToolScope` is **always** recomputed fresh from Layer 1 state (`policy_for(state) -> ActionPolicy`, which produces one `ToolScope` per tool) — it is never itself a stored, independently-mutated object. When a budget is exhausted, the *next* recomputation of `ToolScope` from the (now-updated) Layer 1 state naturally reflects that — there is no separate "mutate `ToolScope` into `Disabled`" step.

**Type invariant:** `AccessLevel::Bounded` requires at least one effective constraint or a finite budget. Without this, a value like `ToolScope { access: Bounded, constraints: { allowed_paths: None, allowed_units: None }, budget: Unlimited }` is constructible but semantically indistinguishable from `Unrestricted` — a silently meaningless "bounded" scope. `ToolScope`'s fields should not be publicly constructible directly; only through smart constructors that make this invariant impossible to violate:

```rust
impl ToolScope {
    fn unrestricted() -> Self;
    fn disabled() -> Self;
    fn bounded_paths(paths: HashSet<PathBuf>, budget: ScopeBudget) -> Self;
    fn bounded_units(units: HashSet<ProjectUnit>, budget: ScopeBudget) -> Self;
}
```

**Budget consumption semantics** (tightened to remove ambiguity):

```
Validation  →  Dispatch  →  Outcome
```

- **Validation failure** (scope violation, invalid arguments, authorization failure): budget is **not** consumed.
- **Dispatch succeeds** (the tool call is actually invoked): budget **is** consumed, regardless of pass/fail outcome.
- **Outcome** (pass/fail/error) affects `Progress`/`Recovery` classification, **not** budget accounting.

This makes bounded verification deterministic: a test run that executes and fails still consumes its budget slot — a model cannot rerun a failing check indefinitely just because it never "succeeds."

### 5.3 Context Snapshot

Prompt/context construction is a Layer 2 derivation, not hand-assembled text appended ad hoc (as today's `goal_with_plan` does). `derive_context_snapshot(state) -> ContextSnapshot` produces: user objective, evidence summary, current capability/scope explanation, the single most recent failure/recovery reason (replacing repeated raw denial text across turns — see `design-report.md` §11), and the current plan. This treats prompt construction with the same discipline as every other policy: it is recomputed from Layer 1, never incrementally patched.

### 5.4 `DisplayPhase` and `CompletionEligibility`

```rust
fn display_phase(state: &AgentState) -> DisplayPhase;           // Explore | Act | Verify | Finish — UI/reporting only
fn completion_eligibility(state: &AgentState) -> CompletionEligibility;
```

```rust
enum CompletionEligibility {
    NotEligible,
    Verified,
    UnverifiedAfterEdit,
    BlockedWithoutEdit,
    NoProgress,
}
```

Neither is ever stored. The enforcement path never asks "what phase am I in" — it asks `policy_for(state)` and `completion_eligibility(state)`.

## 6. Layer 3 — Action evaluation

The conceptual pipeline is `Validate → Authorize → Execute → Classify`, but this cannot be a single pure function: `Execute` has workspace side effects and is async, and `Classify` needs the resulting tool output. A single `evaluate_action(state, action) -> ActionDecision` signature would misrepresent this — it would imply a pure function can complete a pipeline that includes dispatch. The contract is split into four explicit steps instead, so the "only the last step commits Layer 1" constraint is enforceable in code, not just a convention to remember:

```rust
// Pure. No side effects, no I/O. Runs before anything touches the workspace.
fn evaluate_action(state: &AgentState, action: &Action) -> PreDispatchDecision;

enum PreDispatchDecision {
    Dispatch(AuthorizedAction),      // passed Validate + Authorize; safe to execute
    Reject(ActionFailure),           // never reaches Execute
    RequestApproval(ApprovalRequest),// suspends the turn pending a human decision
    Complete(CompletionReason),      // no dispatch needed; run can end here
}

// Impure. The only place workspace side effects happen.
async fn execute_action(action: AuthorizedAction) -> RawToolOutcome;

// Pure. Turns a raw tool result into a structured judgment.
fn classify_outcome(
    state: &AgentState,
    action: &AuthorizedAction,
    outcome: RawToolOutcome,
) -> ClassifiedOutcome;

// The only function permitted to mutate Layer 1.
fn decide_next_state(state: &AgentState, outcome: TurnOutcome) -> RuntimeTransition;

enum TurnOutcome {
    Executed(ClassifiedOutcome), // came from execute_action + classify_outcome
    Rejected(ActionFailure),     // came directly from evaluate_action, nothing ran
}

enum RuntimeTransition {
    Continue,
    Recover(RecoveryAction),
    Complete(CompletionReason),
    Terminate(StopReason),
}

struct ActionFailure {
    kind: ActionFailureKind,
    message: String,
    recoverable: bool,
    allowed_next_actions: Vec<ToolName>,
}
```

`evaluate_action` owns: tool availability, scope validation, argument validity, duplicate-evidence detection, recovery-target restrictions, verification-unit restrictions. **Authorization stays a distinct stage** inside `evaluate_action` — "permitted by runtime state" and "approved by the user" are different concerns and must not be collapsed into one check, matching the orthogonality already correctly present in today's codebase (`design-report.md` §2.11).

`execute_action` performs the real dispatch (workspace side effects) and nothing else — it does not know about scopes, phases, or recovery. `classify_outcome` turns its raw result into the structured judgment (`StallKind`, evidence delta, mutation-committed, etc.). `decide_next_state` is the single point where that judgment (or a pre-dispatch rejection, via `TurnOutcome::Rejected`) is allowed to become a Layer 1 mutation — this is where `RuntimeTransition::Recover` applies one of the recipes in §7.2, and it is the only place any of the boxes in the Layer 3/Layer 4 diagram in §3 actually writes to Layer 1.

## 7. Layer 4 — Runtime decision and the stall model

### 7.1 One counter, tagged, with history

```rust
struct ProgressState {
    consecutive_stalls: u32,
    stall_history: VecDeque<ActionFingerprint>, // capacity 3-4
}

enum StallKind {
    DuplicateEvidence,
    ScopeViolation,
    InvalidArguments,
    FailedMutation,
    RepeatedVerification,
    NonResolvingResponse,
}
```

Only `consecutive_stalls` drives escalation thresholds — this preserves "one number governs termination," which is the actual simplification goal, without discarding the discriminant needed to choose a sensible recovery response. `stall_history` (not a single last-fingerprint scalar) lets recovery logic distinguish:

```rust
is_repeat()     // A, A, A — stuck on one mistake
is_ping_pong()  // A, B, A, B — oscillating between two invalid actions
is_new_failure()// each attempt distinct — a genuinely hard case, closer to a real blocker
```

A single scalar cannot detect ping-ponging (fingerprint never equals "last" even though nothing is progressing); a small ring buffer catches this at negligible state cost.

### 7.2 Recovery recipes (bounded, generic — no per-fixture logic)

| Trigger | Recipe |
|---|---|
| Missing evidence (request for a read of a known-relevant, unseen file) | Grant one exact read via `ToolScope` budget; return to edit-focused policy afterward |
| Failed patch | Grant one exact reread of the failed target; then `apply_patch` or `finish` only |
| Invalid arguments | Return the schema error; permit one same-tool retry; then require a different action |
| Repeated verification | Return the cached prior result; require `apply_patch` or `finish` next |
| Unsupported capability request (tool genuinely disabled, no grant policy matches) | Summarize available evidence via the Context Snapshot; require `apply_patch`, `update_plan`, or `finish` |

Each recipe is required to satisfy the invariant in §7.3 by construction — none of them repeats an identical rejection.

### 7.3 Runtime invariants (final)

1. **Change-or-terminate.** Every runtime intervention must either change what's available to the model on its next turn (capability, scope, context, recovery target, completion state) **or** move the run measurably closer to termination. This is the primary invariant — stronger than "don't reject the same action twice," because it also rules out an implementation satisfying the letter of that rule by changing something irrelevant forever. Any new runtime feature should be checked against this invariant directly: *"which branch does this satisfy?"* If neither, it likely should not exist.
2. **Derive policy from state, never derive state from policy.** `ActionPolicy`, `ToolScope`, `DisplayPhase`, and `CompletionEligibility` are always computed fresh from Layer 1; none of them is a cached or independently-mutated object.
3. **A failed action enters a bounded recovery path**, defined by one of the recipes in §7.2 — never an open-ended retry and never a hand-written, model-wording-specific heuristic (this is the direct replacement for `is_escape_finish`, `design-report.md` §2.7).
4. **Successful edits cannot return to unrestricted exploration without a new, distinct failure** (a new patch or verify failure) tied to the same edit target.
5. **Verification attempts are bounded per project unit**, and a failing-but-executed check still consumes its bound (§5.2's dispatch-based budget rule) — no accidental infinite retry via "it never succeeded."
6. **The runtime can complete a task without a voluntary `finish`.** One `resolve_completion` function (not seven scattered paths) decides this, driven by `consecutive_stalls` and `Verification` state.
7. **The model always retains a safe, non-editing exit** — `finish` remains available in every capability set unconditionally.

## 8. Implementation slices — hypothesis + definition of done

Each slice states one falsifiable architectural question first, then the concrete engineering deliverable. A slice is not "done" because code was written; it is done when its hypothesis has been checked against tests and the eval corpus. Slices are sequential and each one's benchmark evidence should be recorded before starting the next — in particular, Slice 3 (shadow mode) and Slice 4 (cutover) must not be merged into one change: shadow mode exists specifically to compare the new counter against the old ones on real transcripts before anything depends on it.

### Slice 1 — Schema stability for `read_file`

**Hypothesis:** does keeping `read_file` visible-but-scoped in `Act` (instead of removed from the schema) reduce phantom-tool-call failures, without reopening the duplicate-read thrash that caused it to be removed in the first place?

**Scope:**
- Put `read_file` back in `Act`'s schema with `ToolScope { access: Bounded, constraints: { allowed_paths: seen_files }, budget: Unlimited }`.
- One additional `ToolScope` for patch recovery: `{ access: Bounded, constraints: { allowed_paths: {failed_path} }, budget: Uses(1) }`.
- **`is_escape_finish` is left unchanged in this slice** — do not remove or modify it here. Removing it at the same time as the schema/scope change would confound the experiment: if benchmark behavior shifts, there would be no way to attribute the shift to schema stability versus a completion-behavior change. It is removed only in Slice 5, once structured `finish` (§8, Slice 5) actually replaces the need for it.
- Unseen-file reads in `Act` are rejected via a uniform `ActionFailure`, not a phase-specific message.
- `search_code`, `find_files`, and unrestricted shell remain schema-removed in `Act` for this slice (explicitly not yet converted — see §5.1 note).
- Existing stall/denial counters are **temporarily retained as-is** for this slice — Slice 1 does not yet implement invariant §7.3.1 for scope violations; that is Slice 4's job. Success criteria for Slice 1 are about schema stability only.

**Definition of done:** unit tests for `ToolScope` construction and read-request evaluation in both branches (seen-file allowed, unseen-file rejected, recovery-exact-path allowed); `annotaa` AGENT_ISSUES #2 and #5 rerun with no regression; a new synthetic eval where a model asks for a previously-seen file in `Act` completes without triggering the old denial ladder.

### Slice 2 — Unified action evaluator

**Hypothesis:** can one `evaluate_action` function replace the overlapping validators (`action_policy::validate_action`, `loop_engine::validate_action`, and the scattered gate calls) without behavior loss?

**Scope:** consolidate phase/tool/scope/argument validation behind `evaluate_action`; keep `Authorize` as a distinct internal stage; rename or remove the naming collision between the two current `validate_action` functions.

**Definition of done:** every existing gate-pipeline test in `loop_engine.rs` and `action_policy.rs` passes through the new single entry point with identical externally-observed outcomes; no call site outside `evaluate_action` performs its own tool-availability or scope check.

### Slice 3 — Tagged stall counter (shadow mode)

**Hypothesis (revised — see corpus finding below):** can one tagged `consecutive_stalls` counter replace the four overlapping **general** progress/denial counters (`no_progress`, `consecutive_no_progress`, `consecutive_denials`, `consecutive_invalid_actions`), while subsystem-specific bounded state (`Recovery`, `Verification`, `Authorization`) remains authoritative in its own subsystem rather than being folded into the same counter?

The original framing of this hypothesis — "can one counter reproduce the termination points of today's four independent counters" — was corrected mid-implementation after a real corpus run (Issue 3, date/status rules; `run_c56c2372...`) showed why "one counter for everything" is too broad, not just too ambitious:

- `consecutive_stalls` correctly reset to `0` on a genuine recovery-granted reread after a failed `apply_patch` — that reread *was* real new evidence, so the reset was right.
- But at that exact moment, `PatchRecoveryState::consecutive_failures` (a fifth, independent stop mechanism not among the four named above, threshold `MAX_CONSECUTIVE_PATCH_FAILURES = 3`) was sitting at 2-of-3 toward its own termination and, correctly, was **not** reset by that same read — the harness stopped one turn later when the third patch failure landed.
- Had Slice 4 already cut `consecutive_stalls` over to authoritative and retired `PatchRecoveryState` outright, this run would **not** have stopped when it did: the read that legitimately zeroed general progress would also have erased all memory of the mutation-failure streak.

The fix is not to make `StallState` preserve patch-failure streaks across a legitimate progress reset — that would make `Progress` (Layer 1) dependent on `Recovery`-specific semantics and re-blur the boundary this whole plan exists to draw. `Progress` answers *"is the agent generally still making progress?"*; `Recovery` answers *"has this specific mutation target repeatedly failed despite a bounded courtesy retry?"* — different questions, correctly living in different Layer 1 fields (§4). The four counters this slice targets are genuinely redundant restatements of the same "general stuckness" fact; `PatchRecoveryState`'s counters are not — they are a local budget attached to a real bounded process, in the same family as `Verification`'s attempt budget and `Authorization`'s grant state.

**Scope:** introduce `ProgressState` (§7.1) running **alongside** the existing four general counters for telemetry only — it must not yet drive any stop decision. Log both old and new counter states, plus `PatchRecoveryState`'s own counters (`recovery.consecutive_failures`, `recovery.remaining_reads`, `recovery.denied_source_writes`) and, when a stop occurs, which subsystem's threshold actually fired (`legacy_stop_source`), on every recorded transcript — added specifically so this class of divergence is visible in the log directly rather than requiring after-the-fact log archaeology to discover, as it did here.

**Definition of done:** replay the full AGENT_ISSUES corpus (and any stored benchmark transcripts) and compare, per run, whether the new unified counter would have reached its threshold at the same point the old *general* counters actually stopped, **and** confirm `Recovery`'s own bounded state (patch-failure streak, denied-source-write count) is never masked by a `Progress` reset. Old *general* counters are removed only after this comparison shows parity (or an explicitly justified, deliberate behavior change) — this is a shadow-mode migration specifically to avoid guessing new thresholds blind. `PatchRecoveryState`'s counters are explicitly **not** in scope for removal here (see Slice 4).

### Slice 4 — Recovery changes the next request

**Hypothesis:** does routing every invalid/unproductive action through one of the bounded recipes in §7.2 satisfy invariant §7.3.1 (change-or-terminate) in practice, replacing today's escalating-but-repetitive denial text?

**Scope:** wire the five recovery recipes into `decide_next_state`'s `RuntimeTransition::Recover` branch (§6); cut the general stall counter over from shadow mode (Slice 3) to authoritative and **remove only the four general counters** it replaces — `RunState.no_progress`, `ProgressState.consecutive_no_progress`, `RunState.consecutive_denials`, `RunState.consecutive_invalid_actions`. Do **not** simply delete `PatchRecoveryState.consecutive_failures` or `denied_source_writes` alongside them (see Slice 3's revised hypothesis above) — migrate their bookkeeping into the unified Layer 1 `Recovery` field (§4) so the bounded patch-retry/write-denial budget survives as its own authoritative subsystem state, not as an input to the general stall counter.

**Definition of done:** a targeted test per recipe asserting the *second* occurrence of the same trigger never produces byte-identical guidance to the first; full AGENT_ISSUES regression; the `read_file`-after-`Act` failure from the original brief completes with either a successful edit or a well-formed `finish` blocker, never a bare `NoProgress` after N identical denials; **and** a targeted regression for the Slice 3 finding above — a successful patch-recovery reread must reset `Progress.consecutive_stalls` while leaving `Recovery.consecutive_failures` unchanged, and a third consecutive mutation failure on the same target must still terminate via `Recovery`, independent of what `Progress` reads at that moment.

### Slice 5 — Structured `finish`

**Hypothesis:** does a typed `finish` payload, validated against recorded facts, eliminate the need for any prose-based judgment of the model's summary (replacing both `is_escape_finish` and the structural "mentions a seen file" heuristic considered and rejected during design review)?

**Scope:**
```rust
enum FinishStatus { Completed, Blocked, NoChangeNeeded }
struct FinishArgs {
    status: FinishStatus,
    summary: String,
    blocker: Option<String>,
    changed_files: Vec<PathBuf>,
    verification: Option<String>,
}
```
Runtime validates `changed_files` against the actual diff, `Completed` requires a real mutation or explicit no-change justification, `Blocked` requires no completed-modification claim, and any verification claim is checked against `Verification` (Layer 1) rather than trusted as prose. This is a larger, cross-cutting slice (provider tool schemas, benchmark parsing, `require_verified_finish` compatibility) and is deliberately sequenced after Slices 1–4 land, not bundled with them.

**This is also where `is_escape_finish` is finally removed** (kept unchanged since Slice 1, §8 Slice 1) — structured `FinishArgs` makes it unnecessary: a `Blocked` status with no `changed_files` and a `blocker` string is validated structurally, so there is nothing left for a prose heuristic to detect.

**Definition of done:** no eval case can produce a `TaskComplete` result whose claimed `changed_files` disagree with the actual git diff; benchmark scoring pipeline updated and re-validated against the existing scored corpus with no silent scoring drift.

### Slice 6 — Unified completion

**Hypothesis:** does one `resolve_completion` function reproduce (or deliberately improve) the seven completion paths enumerated in `design-report.md` §2.9?

**Scope:** collapse idle-after-finish, post-edit-idle, auto-finish-after-verify, and the three invalid-action-fallback completion routes into `resolve_completion(state) -> Option<StopReason>`, built on `CompletionEligibility` (§5.4) and structured `finish` (Slice 5).

**Definition of done:** `RunResult.stop_reason` distribution unchanged (or changed with explicit justification) across the full existing benchmark corpus before this replaces the old paths.

## 9. What this plan deliberately defers

- Extending `Disabled`-while-present scoping to `search_code`/`find_files`/shell (noted in §5.1 and §8 Slice 1) — real work, not yet designed in detail.
- The exact numeric thresholds for `consecutive_stalls` — intentionally left to Slice 3's shadow-mode comparison rather than guessed here.
- Whether the optional semantic-governance system (`SEMANTIC_POLICY`, default off) becomes redundant with this model — flagged in `design-report.md` §17 as worth revisiting once this plan ships, not decided now.

## 10. Relationship to prior documents

- `annotaa/Gaal.txt` — the original task brief this work answers.
- `annotaa/design-report.md` — the factual investigation of Kairo and Pi (§1–§6, §11–§13, §16–§18 remain authoritative background); its §7–§10 and slice plan are superseded by this document.
- This document (`final-design-plan.md`) — the accepted architecture. Next step is implementation of Slice 1, followed by benchmark evidence, not further design iteration.
