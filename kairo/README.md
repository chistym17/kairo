# Kairo

Terminal coding agent for a local repo — [kairo.so](https://kairo.so)

**Ask** questions (read-only) or run **agent** mode to edit with approval, then verify.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/chistym17/agent-harness/main/install.sh | sh
```

Or: `cargo install kairo`

## Run

```bash
cd your-repo
kairo                              # agent (confirms before first edit)
kairo ask "Where is auth handled?"
kairo agent --task "Fix failing tests"
```

First run prompts for an API key (DeepSeek recommended). Config: `~/.config/kairo/config.toml`

| Mode | Edits |
|------|--------|
| `ask` | no |
| `agent` | yes, after confirm |
| `--edit-policy auto` | yes, no prompts |

## More

[Limitations](https://github.com/chistym17/agent-harness/blob/main/docs/KNOWN_LIMITATIONS.md) · [Changelog](CHANGELOG.md) · [Releases](https://github.com/chistym17/agent-harness/releases)
