# Kairo v2 — release checklist

Run before publishing a version (crates.io + GitHub Release binaries).

## Preflight

- [ ] `kairo/Cargo.toml` version matches intended release (e.g. `0.2.1`)
- [ ] `kairo/CHANGELOG.md` has a section for that version
- [ ] README install section still leads with the install script / Releases path
- [ ] `docs/KNOWN_LIMITATIONS.md` present and linked from README
- [ ] No secrets in the package: `cargo package -p kairo --list` (no `.env`, no fixture oracles, no session logs)

## Package dry-run (crates.io)

```bash
cd /path/to/agent-harness
cargo package -p kairo --allow-dirty   # or clean tree without --allow-dirty
cargo package -p kairo --list | grep -E '\.env|oracle|sessions/|logs/' && echo 'FAIL: junk in package' || echo 'OK'
```

## Publish crates.io

```bash
cargo publish -p kairo
```

## GitHub Release binaries (install script)

Tagging triggers `.github/workflows/release.yml`, which builds Linux/macOS binaries and attaches them to the Release (what `install.sh` downloads).

```bash
git tag v0.2.1
git push origin v0.2.1
# watch: https://github.com/chistym17/agent-harness/actions
```

After the workflow finishes:

```bash
curl -fsSL https://kairo.so/install.sh | sh
kairo --version   # or: kairo health
```

Manual rebuild for an existing tag: Actions → Release → Run workflow → set tag (e.g. `v0.2.1`).

Also sync site install script when shipping: copy root `install.sh` → `frontend/install.sh` (served at `https://kairo.so/install.sh`).

## Smoke (fresh install path)

```bash
# binary path
curl -fsSL https://kairo.so/install.sh | sh
# or: cargo install kairo --force

kairo health
cd /path/to/some/git/repo
kairo ask "Give a one-paragraph overview of this repo."
# interactive: confirm one harmless edit, or reject it
kairo agent --edit-policy confirm --task "Add a TODO comment in README only if appropriate — otherwise explain why not."
```

## After publish

- [ ] `cargo install kairo --force` from crates.io on a clean machine / CI
- [ ] install script works against the new Release assets
- [ ] Update [kairo.so](https://kairo.so) blurb if needed
- [ ] Note recommended model: DeepSeek `deepseek-v4-flash`
