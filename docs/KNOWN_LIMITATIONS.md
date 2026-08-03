# Kairo — known limitations (v2)

Honest limits for operators. Measured mainly with `deepseek-v4-flash` on local fixtures (2026-08).

## What Kairo is good at

- Read-only **ask** over a real repo
- **Scoped** bugfixes (wrong field, inverted check, one-file contract mismatch)
- Interactive **confirm** edits with a reviewable diff
- Stopping honestly when verify cannot run (`CompletedUnverified` / environment blocked)

## What still fails often

| Limitation | Symptom | Advice |
|------------|---------|--------|
| **Act thrash** | Finds the right file, re-reads / denied tools, stops with **no edit** (`NoProgress`) | Narrow the task; point at the file; retry once |
| **Wrong target** | Edits a related but wrong module | Be explicit in the prompt; review `git diff` |
| **Multi-file / new-file work** | Partial fixes or `apply_patch` on missing paths | Split tasks; create files yourself when needed |
| **Headerless first patch** | First `ApplyPatch` rejected, second succeeds | Normal; usually self-recovers in one turn |
| **Verify environment** | Lint/`next`/pytest missing → blocked or timed out | Install project tools, or treat finish as “edit landed, verify locally” |
| **Provider flakes** | DNS / API errors mid-run | Retry; check network and API key |

## Safety

- Default edit policy is **`confirm`** — approve before the first write.
- **`auto`** applies edits without prompting — use only in trusted repos / CI you control.
- **`ask`** cannot write.
- The agent can run shell commands inside the workspace; do not point it at secrets-only trees or untrusted checkouts without review.

## Eval baselines (blind, approx.)

| Fixture | Solid pass rate |
|---------|----------------:|
| VerdictAI | ~90% (+1 near-miss) |
| CodeHack | ~80% |
| AnimalHack | ~70% |
| Annotaa (harder) | ~50% |

These are regression baselines for the runtime, not a guarantee on your private monorepo.

## Related

- Release checklist: [`RELEASE_CHECKLIST.md`](./RELEASE_CHECKLIST.md)
- Changelog: [`../kairo/CHANGELOG.md`](../kairo/CHANGELOG.md)
