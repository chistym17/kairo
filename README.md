# Kairo

Local terminal coding agent — [kairo.so](https://kairo.so)

Ask questions about a repo, or let it plan/edit/verify with your approval.

**Good for:** scoped bugfixes, repo Q&A  
**Not for:** unattended autopilot  
**Model:** DeepSeek [`deepseek-v4-flash`](https://platform.deepseek.com) (recommended)

## Features

- **Ask** — read-only Q&A over the workspace
- **Agent** — explore → edit → verify (default: confirm before first edit)
- **Safety** — workspace-scoped tools, edit policies, approval prompts
- **TUI** — full-screen terminal UI for interactive sessions

## Install

```bash
curl -fsSL https://kairo.so/install.sh | sh
```

Puts `kairo` in `~/.local/bin` ([Releases](https://github.com/chistym17/kairo/releases)). No Rust needed.

```bash
cargo install kairo   # optional, if you already use Rust
```

## Setup

First run asks for provider + API key (saved to `~/.config/kairo/config.toml`).

Or set env:

| Provider | Env |
|----------|-----|
| DeepSeek | `LLM_PROVIDER=deepseek` + `DEEPSEEK_API_KEY` |
| Groq | `LLM_PROVIDER=groq` + `GROQ_API_KEY` |

## Use

```bash
cd your-repo
kairo                              # interactive agent
kairo ask "Where is auth handled?"
kairo agent --task "Fix failing tests"
kairo health
kairo config
```

| Mode | Edits |
|------|--------|
| `ask` | never |
| `agent` / `kairo` | confirm first edit (default) |
| `--edit-policy auto` | no prompts (trusted/CI only) |

Approvals: **Enter**/`t` = for task · `a` = once · `r` = reject

## Docs

- [Changelog](kairo/CHANGELOG.md)
- [Limitations](docs/KNOWN_LIMITATIONS.md)
- [Release checklist](docs/RELEASE_CHECKLIST.md)
