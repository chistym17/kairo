# Phase v3 — Generic primitives (ordered)

Philosophy from phase v2 still holds: **govern environment, resources, protocol, and safety — not model reasoning.**

Build generic backends behind thin seams. Gate each seam on **two concrete backends** before expanding.

## Priority order

1. **Provider-independent model protocol** ← start here
   - Normalize every provider turn to `AgentResponse::{ ToolCalls | FinalAnswer | ProtocolError }`
   - Provider adapters own wire format; the loop never branches on Groq/DeepSeek/AgentRouter specifics
   - Existing `next_actions` stays as a thin legacy adapter so the loop need not change yet

2. **Generic verification / check discovery** — passive recognition implemented
   - Discover project checks objectively (build, test, lint) without teaching the model “when to verify”
   - Match model-run commands to discovered checks and record structured outcomes
   - Automatic check execution remains intentionally deferred

3. **Thin RepositoryIntelligence**
   - Wrap existing search / file / git tools; no new symbol backends until the seam exists

4. **Richer symbol backends** (second RI backend)
   - Only after RI is used by two backends

5. **Context providers / batching**
   - Last — after protocol and verification are stable

## Out of scope for this phase

- Semantic governance / mode machines
- Changing the core tool loop semantics beyond consuming the normalized protocol
