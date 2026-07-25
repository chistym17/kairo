# Cache verification matrix (Phase 13A Day 5)

Measure first. Success is answering these confidently — not hitting a target %.

```text
What did each call cost?
How much input was cached? (or unavailable)
Did our stable prefix change?
If it changed, which component? (tools / system / instructions / session)
Did appending state preserve earlier messages?
```

## Local (no API)

```bash
cd kairo
cargo test --lib cache_verify
```

Covers harness expectations for matrix **A**, **C**, **D**.

## Live matrix

From `kairo/` with `.env` configured:

### A — Consecutive turns, same session

```bash
cargo run --release -- --workspace /path/to/repo
```

In one interactive session, send 2–3 short turns that do **not** change tools/session
(e.g. “list top-level files”, then “how many files did you see?”).

Watch:

| Signal | Expected |
|--------|----------|
| `[prompt] stable=` | Same across turns (until a Session update) |
| `[cache-verify] prefix=Unchanged` | Yes |
| `[usage] cached=` | Rise on later turns **if** provider reports cache; else `unavailable` |

### B — Fresh identical session

Exit, start a **new** session with the same workspace/goal.

| Signal | Expected |
|--------|----------|
| Cached tokens | May hit or miss (provider lifetime / routing) |
| Do not treat a miss as harness churn if `stable=` matches a prior run’s first-turn hash |

### C — Appended policy update

```text
/edit-policy auto
```

Then continue the task.

| Signal | Expected |
|--------|----------|
| Earlier messages | Unchanged (Session update appended) |
| `[cache-verify] prefix=SessionChanged` | tools/system still false |
| Provider | Earlier prefix may still be cacheable as a shared prefix |

### D — Tool schema change

Requires a code change to a tool description (or temporary schema edit), then rebuild and rerun.

| Signal | Expected |
|--------|----------|
| `[cache-verify] prefix=ToolsChanged` | Yes |
| Cached tokens | Miss from tools section onward (when reported) |

## Record findings

After a live pass, note:

```text
Provider / model:
Date:

A. consecutive turns — stable hash: same/diff ; cached: …
B. fresh session — cached: …
C. after /edit-policy — prefix reason: … ; tools unchanged: …
D. tool schema (if run) — prefix reason: …

Notes (unavailable vs 0, surprises):
```

If cached stays `unavailable`, use `[cache-verify]` / `[prompt]` hashes to decide whether the harness is stable before building 13B/13C local caches.
