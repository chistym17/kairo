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

Optional: `MODEL_ID`. Full list: [`agent-v1/.env.example`](agent-v1/.env.example).

Never commit `.env` — it is gitignored.

### 3. Run

From your project (workspace is auto-detected from the nearest `.git` root):

```bash
cd /path/to/your/repo
agent-v1                              # interactive agent
agent-v1 ask "Where is authentication handled?"
agent-v1 agent --edit-policy auto --task "Fix the failing tests"
agent-v1 serve --port 8787            # → http://127.0.0.1:8787
```

Override with `--workspace /other/path` when needed. Without `cargo install`, use `cargo run -p agent-v1 --release --` from the harness repo (load `.env` from `agent-v1/` or export keys).

Startup prints:

```text
Workspace: ~/projects/shop
Branch: main
```

## What to try first

1. `cd` into a repo and run `agent-v1`
2. **Ask:** `What's in this repo? Give a short overview.`
3. **Ask:** `Why might the tests be failing?` (no edits)
4. **Agent:** `Fix the failing tests.` — approve the first edit with **Enter** / `t`
5. **Serve:** open the UI, toggle Ask ↔ Agent, send the same prompts

## Modes and safety

| Command | Default edit policy | Behavior |
|---------|---------------------|----------|
| `agent-v1` / `agent` | `confirm` | Pause before the first workspace edit |
| `ask` | always `readonly` | Explore and answer; cannot write |
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
| Wrong workspace | `cd` to the repo, or pass `--workspace` |
| `workspace does not exist` | Path must be an existing directory |
| Missing API key | Set the key for your `LLM_PROVIDER` (see table above) |
| `.env` ignored | Run from a dir with `.env`, or export vars; `dotenv` loads cwd `.env` |
| Edits blocked in oneshot + confirm | Use interactive/`serve`, or `--edit-policy auto` |
| Port in use | `serve --port 8790` |

## More

- Agent package notes: [`agent-v1/README.md`](agent-v1/README.md)
- Prompt-cache verification: [`agent-v1/CACHE_VERIFY.md`](agent-v1/CACHE_VERIFY.md)
- Portfolio V1 plan: [`final-plan.md`](final-plan.md)
