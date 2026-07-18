# Phase v3 — Generic primitives (ordered)

Philosophy from phase v2 still holds: **govern environment, resources, protocol, and safety — not model reasoning.**

Build generic backends behind thin seams. Gate each seam on **two concrete backends** before expanding.

## Priority order

1. **Provider-independent model protocol** ← start here
   - Normalize every provider turn to `AgentResponse::{ ToolCalls | FinalAnswer | ProtocolError }`
   - Provider adapters own wire format; the loop never branches on Groq/DeepSeek/AgentRouter specifics
   - Existing `next_actions` stays as a thin legacy adapter so the loop need not change yet

2. **Generic verification / check discovery** — passive recognition + env handling
   - Discover project checks objectively (build, test, lint) without teaching the model “when to verify”
   - Match model-run commands to discovered checks and record structured outcomes
   - Rank preferred checks: typecheck → lint → test → build; demote browser/e2e commands
   - Classify sandbox/browser crashes as `EnvironmentBlocked` (not patch failure)
   - Automatic check execution remains intentionally deferred

3. **Thin RepositoryIntelligence** — Phase A+B: single `locate` tool
   - `RepositoryIntelligence::locate` + `SearchSnapshotIntel` backend
   - Additive tool; raw `search_code` retained
   - Defer `RepoContext` / AST until locate proves insufficient

4. **Richer symbol backends** (second RI backend)
   - Only after RI is used by two backends

5. **Context providers / batching**
   - Last — after protocol and verification are stable

## Out of scope for this phase

- Semantic governance / mode machines
- Changing the core tool loop semantics beyond consuming the normalized protocol



 "On /tasks, selecting a day shows no tasks even though tasks exist for that date. The list endpoint expects a specific query parameter name. Find the frontend/backend mismatch in the tasks API client and fix it. Verify with backend tasks tests."
