# Kairo

Local coding agent ([kairo.so](https://kairo.so)) — **v2**.

Point it at a repo, **ask** read-only questions, or run **agent** mode to plan, edit (with approval), and verify — from the terminal.

**Best at:** scoped bugfixes and repo Q&A.  
**Not:** unattended production Autopilot. Default edit policy is **confirm** — review diffs before you merge. Use `--edit-policy auto` only in trusted repos or CI you control.

**Recommended model (eval-backed):** DeepSeek [`deepseek-v4-flash`](https://platform.deepseek.com) (`LLM_PROVIDER=deepseek`).

## Five minutes to first run

### 1. Install (no Rust required)

```bash
curl -fsSL https://raw.githubusercontent.com/chistym17/agent-harness/main/install.sh | sh
```

That downloads the latest [GitHub Release](https://github.com/chistym17/agent-harness/releases) binary into `~/.local/bin`. If that directory is not on your `PATH`, the script prints the export line to add.

Pin a version:

```bash
KAIRO_VERSION=v0.2.1 curl -fsSL https://raw.githubusercontent.com/chistym17/agent-harness/main/install.sh | sh
```

**Or with Rust / cargo:**

```bash
cargo install kairo
# or: cargo install --git https://github.com/chistym17/agent-harness --bin kairo
```

### 2. API key

On first run, `kairo` prompts for provider, API key, and model, then saves
`~/.config/kairo/config.toml` (mode `0600`). After that, `cd` any repo and run
without a project `.env`.

To override (or for CI/dev), export env vars or use a local `.env`:

```bash
# from a clone:
cp kairo/.env.example kairo/.env
```

| Provider | Env |
|----------|-----|
| [DeepSeek](https://platform.deepseek.com) (recommended) | `LLM_PROVIDER=deepseek` + `DEEPSEEK_API_KEY=…` |
| [Groq](https://console.groq.com) | `LLM_PROVIDER=groq` + `GROQ_API_KEY=…` |
| [AgentRouter](https://agentrouter.org/console/token) | `LLM_PROVIDER=agentrouter` + `AGENTROUTER_API_KEY=…` |

Optional: `MODEL_ID` (DeepSeek default: `deepseek-v4-flash`). Priority: env / `.env` → user `config.toml` → defaults.
Never commit secrets.

### 3. Run

From your project (workspace is auto-detected from the nearest `.git` root):

```bash
cd /path/to/your/repo
kairo                              # interactive agent (confirm before first edit)
kairo ask "Where is authentication handled?"
kairo agent --edit-policy confirm --task "Fix the failing tests"
kairo agent --edit-policy auto --task "…"   # no approval prompts — power users / CI only
kairo config                       # change provider / model / API key
kairo health                       # check config and workspace readiness
```

Override with `--workspace /other/path` when needed. Without `cargo install`, use `cargo run -p kairo --release --` from the harness repo (load `.env` from `kairo/` or export keys).

Startup prints:

```text
Workspace: ~/projects/shop
Branch: main
```

## What to try first

1. `cd` into a repo and run `kairo`
2. **Ask:** `What's in this repo? Give a short overview.`
3. **Ask:** `Why might the tests be failing?` (no edits)
4. **Agent:** `Fix the failing tests.` — approve the first edit with **Enter** / `t`

## Modes and safety

| Command | Default edit policy | Behavior |
|---------|---------------------|----------|
| `kairo` / `agent` | `confirm` | Pause before the first workspace edit |
| `ask` | always `readonly` | Explore and answer; cannot write |
| `agent --edit-policy auto` | `auto` | Apply edits without prompting |

Approvals (terminal): **Enter** / `t` = approve for task · `a` = once · `r` = reject.

The agent can run shell commands **inside the workspace**. Do not aim it at secret-only trees or untrusted checkouts without watching the session.

### Verification honesty

After an edit, Kairo may run project checks (lint / test / build). If tools are missing or the environment blocks them, the run can finish as **`CompletedUnverified`** or show an environment-blocked check. That means **the edit landed; local verify still needs you** — not that production tests passed.

Machine-readable events:

```bash
kairo ask --workspace ./repo --output jsonl "…"
```

## Embed as a library

```rust
use kairo::api::{AgentConfig, AgentEvent, AgentMode, AgentSession, EditPolicy};

// see kairo/examples/basic_agent.rs
```

```bash
cargo run -p kairo --example basic_agent -- --ask /path/to/repo "…"
```

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| Wrong workspace | `cd` to the repo, or pass `--workspace` |
| `workspace does not exist` | Path must be an existing directory |
| Missing API key | Run `kairo` once for the setup wizard, or set `*_API_KEY` / edit `~/.config/kairo/config.toml` |
| `.env` ignored | Env overrides user config; `dotenv` loads cwd `.env` — or rely on `config.toml` |
| Edits blocked in oneshot + confirm | Use interactive mode, or `--edit-policy auto` |
| Stopped with no edit / thrash | Narrow the task or name the file; see [known limitations](docs/KNOWN_LIMITATIONS.md) |

## More

- Agent package notes: [`kairo/README.md`](kairo/README.md)
- Changelog: [`kairo/CHANGELOG.md`](kairo/CHANGELOG.md)
- Known limitations: [`docs/KNOWN_LIMITATIONS.md`](docs/KNOWN_LIMITATIONS.md)
- Release checklist: [`docs/RELEASE_CHECKLIST.md`](docs/RELEASE_CHECKLIST.md)
