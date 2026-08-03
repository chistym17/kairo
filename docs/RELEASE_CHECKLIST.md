# Kairo v2 — release checklist

Run before `cargo publish -p kairo` / tagging a GitHub release. **No runtime code changes required** for a hygiene pass.

## Preflight

- [ ] `kairo/Cargo.toml` version matches intended release (e.g. `0.2.0`)
- [ ] `kairo/CHANGELOG.md` has a section for that version
- [ ] README positioning: confirm default, auto = power user, recommended model noted
- [ ] `docs/KNOWN_LIMITATIONS.md` present and linked from README
- [ ] No secrets in the package: `cargo package -p kairo --list` (no `.env`, no fixture oracles, no session logs)

## Package dry-run

```bash
cd /path/to/agent-harness
cargo package -p kairo --allow-dirty   # or clean tree without --allow-dirty
cargo package -p kairo --list | grep -E '\.env|oracle|sessions/|logs/' && echo 'FAIL: junk in package' || echo 'OK'
```

## Smoke (fresh install path)

```bash
cargo install --path kairo --force
kairo health
cd /path/to/some/git/repo
kairo ask "Give a one-paragraph overview of this repo."
# interactive: confirm one harmless edit, or reject it
kairo agent --edit-policy confirm --task "Add a TODO comment in README only if appropriate — otherwise explain why not."
```

## Publish

```bash
cargo publish -p kairo
# optional: git tag v0.2.0 && git push origin v0.2.0
```

## After publish

- [ ] `cargo install kairo --force` from crates.io on a clean machine / CI
- [ ] Update [kairo.so](https://kairo.so) blurb if it still says pre-v2 wording
- [ ] Note recommended model: DeepSeek `deepseek-v4-flash`
