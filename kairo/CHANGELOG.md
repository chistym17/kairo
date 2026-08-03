# Changelog

All notable changes to the **Kairo** crate are documented here.

## [0.2.0] — 2026-08-03 — Kairo v2 (architecture)

Product release of the Layer-4 runtime redesign. Same install path (`cargo install kairo`); this is the **v2** architecture line for real local use.

### Highlights

- Unified pre-dispatch evaluation and Layer-4 recovery (`Continue` / `Recover` / `Terminate` / `Complete`)
- Safer patch path: bare-hunk normalization when uniquely grounded; invalid syntax ≠ workspace FailedPatch
- Finish-phase auto-complete (no InvalidArguments thrash after the run is already complete)
- Richer verification outcome labels without fake `TaskComplete`
- Ask / agent UX unchanged in spirit: **confirm** remains the default edit policy
- Local web UI (`kairo serve`) removed — CLI only

### Positioning

Kairo v2 is a **local** coding agent: ask read-only questions, or plan/edit/verify in your repo with approvals. It is strong on **scoped, single-site bugfixes**. It is **not** an unattended production Autopilot — review diffs before you merge.

### Recommended model (eval-backed)

- **DeepSeek** `deepseek-v4-flash` — primary model used for the multi-repo blind evals that informed this release

### Docs added for this release

- `CHANGELOG.md` (this file)
- Root / package README: safety, limitations, verify honesty
- `docs/KNOWN_LIMITATIONS.md`
- `docs/RELEASE_CHECKLIST.md`

### Not in this release (deferred on purpose)

- Act-phase thrash redesign
- Stall-policy unified cutover
- 5G grounding-window redesign
- Broad AI-policy / recovery-budget retuning

## [0.1.1] — prior

Initial crates.io-oriented packaging of the interactive agent (ask / agent / serve, edit policies, config wizard).
