# agent-harness

Local coding agent in Rust. Point it at a repo, **ask** read-only questions, or run **agent** mode to plan, edit (with approval), and verify — from the terminal or a localhost UI.

## Five minutes to first run

### 1. Install

**Option A — from GitHub**

```bash
cargo install --git https://github.com/chistym17/agent-harness --bin agent-v1
```

**Option B — from a clone**

```bash
git clone https://github.com/chistym17/agent-harness.git
cd agent-harness
cargo install --path agent-v1
```

You need a stable [Rust toolchain](https://rustup.rs/).

### 2. API key

Create `~/.config/agent-v1/.env` or export vars in your shell. Easiest while developing: copy the example next to the binary’s working directory.

```bash
# from a clone:
cp agent-v1/.env.example agent-v1/.env
```

Edit and set **one** provider:

| Provider | In `.env` |
|----------|-----------|
| [Groq](https://console.groq.com) | `LLM_PROVIDER=groq` + `GROQ_API_KEY=…` |
| [DeepSeek](https://platform.deepseek.com) | `LLM_PROVIDER=deepseek` + `DEEPSEEK_API_KEY=…` |
| [AgentRouter](https://agentrouter.org/console/token) | `LLM_PROVIDER=agentrouter` + `AGENTROUTER_API_KEY=…` |

Optional: `MODEL_ID`, `WORKSPACE_ROOT`. Full list: [`agent-v1/.env.example`](agent-v1/.env.example).

Never commit `.env` — it is gitignored.

### 3. Run

From a directory where `.env` is loadable (e.g. `agent-v1/`), or with vars exported:

```bash
# Ask — read-only, answers from the repo
agent-v1 ask --workspace /path/to/your/repo "Where is authentication handled?"

# Agent — edits; default policy is confirm (you approve the first edit)
agent-v1 agent --workspace /path/to/your/repo

# One-shot change (auto-apply; use only when you trust the run)
agent-v1 agent --workspace /path/to/your/repo --edit-policy auto --task "Fix the failing tests"

# Local web UI (127.0.0.1 only)
agent-v1 serve --workspace /path/to/your/repo --port 8787
# → open http://127.0.0.1:8787
```

Without `cargo install`, prefix with `cargo run -p agent-v1 --release --` from the repo root (or `cd agent-v1 && cargo run --release --`).

## What to try first

1. **Ask:** `What's in this repo? Give a short overview.`
2. **Ask:** `Why might the tests be failing?` (no edits)
3. **Agent:** `Fix the failing tests.` — approve the first edit with **Enter** / `t`
4. **Serve:** open the UI, toggle Ask ↔ Agent, send the same prompts

## Modes and safety

| Command | Default edit policy | Behavior |
|---------|---------------------|----------|
| `ask` | always `readonly` | Explore and answer; cannot write |
| `agent` | `confirm` | Pause before the first workspace edit |
| `agent --edit-policy auto` | `auto` | Apply edits without prompting |
| `serve` | `confirm` (overridable) | Same engine; approvals in the browser |

Approvals (terminal): **Enter** / `t` = approve for task · `a` = once · `r` = reject.

Machine-readable events:

```bash
agent-v1 ask --workspace ./repo --output jsonl "…"
```

## Embed as a library

```rust
use agent_v1::api::{AgentConfig, AgentEvent, AgentMode, AgentSession, EditPolicy};

// see agent-v1/examples/basic_agent.rs
```

```bash
cargo run -p agent-v1 --example basic_agent -- --ask /path/to/repo "…"
```

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `workspace not set` | Pass `--workspace` or set `WORKSPACE_ROOT` |
| `workspace does not exist` | Path must be an existing directory |
| Missing API key | Set the key for your `LLM_PROVIDER` (see table above) |
| `.env` ignored | Run from `agent-v1/`, or export vars; `dotenv` loads cwd `.env` |
| Edits blocked in oneshot + confirm | Use interactive/`serve`, or `--edit-policy auto` |
| Port in use | `serve --port 8790` |

## More

- Agent package notes: [`agent-v1/README.md`](agent-v1/README.md)
- Prompt-cache verification: [`agent-v1/CACHE_VERIFY.md`](agent-v1/CACHE_VERIFY.md)
- Portfolio V1 plan: [`final-plan.md`](final-plan.md)
