# Annotaa eval — runtime v1 (pre-merge)

**Branch:** `architecture-improvement`  
**Date:** 2026-08-02  
**Fixture:** Annotaa (`AGENT_ISSUES.md` / `AGENT_ISSUES.oracle.md`)  
**Model:** `deepseek-v4-flash`  
**Runner:** `./run-issue.sh N` (reset to `buggy` between issues unless noted)  
**Log batch:** `kairo/logs/1_*.jsonl` … `17_*.jsonl` (≈14:27–14:46 local)

This is the **freeze / pre-merge scorecard** for the Layer-4 runtime redesign. It measures harness behavior and oracle-correct fixes — not “agent said it finished.”

---

## Summary

| Metric | Value |
|--------|------:|
| Issues in suite | 10 |
| Clear oracle passes | **5** |
| Partial (right area, incomplete / risky) | **2** |
| Fails | **3** |
| Solid pass rate | **50%** |
| Pass + partial | **~70%** |
| Runs in batch (incl. retries) | 17 |
| Stops: `CompletedUnverified` | 9 |
| Stops: `NoProgress` | 8 |
| Patches applied / rejected | 7 / 24 |

**Verdict:** The runtime redesign is **mergeable as an architecture cut**. Patch mechanics and finish handling are in good shape. Annotaa is **not** “reliably solved”; remaining gaps are mostly model targeting, Act-read thrash, and new-file creation — not bare-hunk / FailedPatch death spirals.

---

## What this runtime slice froze

Accepted on this branch (do not reopen for merge):

| Slice | Intent | Status |
|-------|--------|--------|
| ToolScope + Act `read_file` visibility | Scoped tools without hiding schema | In |
| Unified `evaluate_action` | Single pre-dispatch gate | In |
| Layer 4 recovery recipes | Bounded Recover / Terminate | In |
| Stall policy | Shadow by default (`KAIRO_STALL_POLICY`) | In (no cutover) |
| Verification assessment (5F) | Richer labels; no StopReason upgrade | In |
| Bare-hunk normalization | Unique-context rewrite only | **Frozen** |
| Finish-phase non-finish | Auto-complete; no InvalidArguments thrash | In |

Deferred on purpose (not blocking this merge):

- Unified stall cutover / removing legacy counters  
- 5G grounding-window redesign  
- Further AI-policy / recovery budget tuning  

---

## Suite scorecard (best attempt per issue)

Oracle source: `AGENT_ISSUES.oracle.md`.

| # | Issue | Oracle target | Result | Best log(s) | Notes |
|---|-------|---------------|--------|-------------|-------|
| 1 | Empty board | FE `lib/api.ts` (`dueDate`→`date`) | **Fail** | `1`, `2` | `NoProgress`; never edited the API client — Act thrash |
| 2 | Polygon scale | FE `polygonDrawing.ts` | **Pass** | `3` | `* scale` → `/ scale`; eslint verify |
| 3 | Date rules | FE `task-status.ts` | **Pass** | `5` | Correct invert of future/past predicates |
| 4 | Kanban reorder | BE `tasks/services.py` | **Fail** | `6`–`8` | Edited FE `store.ts` instead of `move_task`; finished with speculation |
| 5 | Triangle validation | BE annotations serializers (`<4`→`<3`) | **Pass** | `9` | Correct one-line fix |
| 6 | Day list filter | BE `tasks/views.py` | **Pass** | `10` | Correct `due_date=` filter |
| 7 | Annotate perf | BE annotations views + serializers | **Partial** | `11` | Prefetch + removed sync loop; also invented pagination (API risk); FE follow-up blocked by Finish |
| 8 | Task mapping | FE `storeMap.ts` | **Pass** | `14` | `due_date` → `dueDate` (after wrong-file retries in `13`) |
| 9 | Activity page | New FE page + nav | **Fail** | `15` | Understood task; `apply_patch` cannot create files; finished as tooling blocker |
| 10 | Stats API + page | New BE endpoint + FE + nav | **Partial** | `17` | Landed `TaskStatsView` only; urls/FE/nav incomplete; Finish auto-complete |

---

## Harness health (this batch)

| Signal | Count | Reading |
|--------|------:|---------|
| `patch_hunk_normalized` | 2 | Safe rewrite reached `mpatch` |
| `patch_bare_hunk_rejected` | 5 | Ambiguous/weak bare hunks → InvalidArguments (not FailedPatch) |
| `No patches were found` | 1 | Nearly eliminated vs pre-normalizer runs |
| `patch_grounded_packet` | 8 | 5G packets issued after real mismatches |
| `patch_ungrounded_reject` | 2 | Grounding gate fired |
| `Phase already complete` / finish auto-complete | 4 / 2 stops | Finish non-finish → Terminate, no retry loop |
| `FailedPatch` mentions (log text) | 65 | Still present; now mostly real dispatched mismatches + recovery chatter |

### What improved vs pre-redesign pain

```text
Before (typical Issue 7 / bare @@):
  correct-looking bare hunk
  → mpatch: “No patches were found”
  → FailedPatch budget burned
  → misleading recovery loop

Now:
  bare hunk
  → normalize when uniquely grounded
  → reject as InvalidArguments when not
  → FailedPatch only after a real dispatched mismatch
```

Finish phase:

```text
Finish eligible → non-finish tool
→ record phase_already_complete
→ auto-complete (CompletedUnverified)
→ no InvalidArguments “fix and retry” recovery
```

### What still burns eval points

1. **Act-phase read thrash → `NoProgress` with zero edits**  
   Common on issues 1, 4, 7, 12. Model retries `locate` / out-of-scope `read_file` while Act still lists `read_file` as allowed — denials feel contradictory, then the run dies without a patch.

2. **Wrong-target diagnosis**  
   Issue 4 never touched `services.py`. Harness cannot compensate for FE-vs-BE misreads.

3. **New-file creation**  
   Issues 9–10 need `write_file`; models often keep calling `apply_patch` on missing paths.

4. **Multi-cause / inventiveness (Issue 7)**  
   Partial perf fixes land; pagination and incomplete serializer work remain model ceiling.

5. **Verification environment**  
   Frequent `verification_blocked` — assessment is honest; does not prove green tests in CI-like conditions.

---

## Run outcome mix

| `stop_reason` | Runs | Meaning in this suite |
|---------------|-----:|------------------------|
| `CompletedUnverified` | 9 | Edit or finish path; verify often blocked or not fully trusted |
| `NoProgress` | 8 | Usually Act thrash / no successful mutation |

| `completion_assessment` | Runs |
|-------------------------|-----:|
| `unverified` | 10 |
| `verification_blocked` | 4 |
| `verified` | 3 |

---

## Merge recommendation

**Ship this branch as runtime v1** if the bar is:

- Layer-4 / ToolScope / unified evaluate / bare-hunk / finish auto-complete are the product deliverable  
- Annotaa is a **regression baseline** (5/10 clear, known hard fails), not a release gate of 10/10  

**Do not block merge on:**

- Solving Issue 7 fully  
- 5G window redesign  
- Stall `unified` cutover  

**After merge (optional, separate):**

1. Keep this scorecard as the Annotaa baseline for `architecture-improvement` → `main`.  
2. Re-run issues **1, 4, 9, 10** when changing Act-read or `write_file` UX.  
3. Consider stall cutover only after shadow logs look calm on real usage.

---

## How to reproduce

```bash
cd annotaa && git reset --hard buggy && cd ..
./run-issue.sh N    # N = 1..10
# Logs: kairo/logs/<seq>_<runid>.jsonl
```

Oracle (human only — do not paste into the agent): `AGENT_ISSUES.oracle.md`.

---

## Log index (this batch)

| Log | Likely issue | Stop | Assessment | Oracle note |
|-----|--------------|------|------------|-------------|
| `1_e23d8e4e` | 1 | NoProgress | unverified | No edit |
| `2_bd64fd8e` | 1 (retry / misread) | NoProgress | unverified | No edit |
| `3_9f02218f` | 2 | CompletedUnverified | verified | **Pass** |
| `4_83dd218f` | 3 (explore) | NoProgress | unverified | No edit |
| `5_86425ac2` | 3 | CompletedUnverified | verified | **Pass** |
| `6_cfb53066` | 4 | NoProgress | unverified | Wrong file |
| `7_efe01c98` | 4 | NoProgress | unverified | No edit |
| `8_bc6039ec` | 4 | CompletedUnverified | unverified | No successful edit |
| `9_ed265c26` | 5 | CompletedUnverified | verification_blocked | **Pass** |
| `10_c7ee9deb` | 6 | CompletedUnverified | verification_blocked | **Pass** |
| `11_8e9018c7` | 7 | CompletedUnverified | verification_blocked | **Partial** |
| `12_5602bdc3` | 8 (explore) | NoProgress | unverified | No edit |
| `13_7da13508` | 8 | NoProgress | unverified | Wrong / noop patches |
| `14_ab9a8543` | 8 | CompletedUnverified | verified | **Pass** |
| `15_64c372d7` | 9 | CompletedUnverified | unverified | New-file blocker |
| `16_28a3a67a` | 10 | NoProgress | unverified | Bare/shape rejects |
| `17_690f588e` | 10 | CompletedUnverified | verification_blocked | **Partial** |
