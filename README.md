# agent-harness

Minimal Rust coding-agent runtime: point it at **any** git repo, explore, plan, edit (with approval), and finish.

## Prerequisites

- Rust toolchain ([rustup](https://rustup.rs/)) — stable
- An LLM API key (Groq, DeepSeek, or AgentRouter)

## Install

```bash
git clone <this-repo-url>
cd agent-harness/agent-v1
cp .env.example .env
# edit .env: set LLM_PROVIDER, API key, and optionally WORKSPACE_ROOT
cargo build --release
```

Optional install:

```bash
cargo install --path .
```

## Run

From `agent-v1/` (so `.env` loads):

```bash
# Interactive — confirm-before-edit by default
cargo run --release -- --workspace /path/to/your/repo

# Or set WORKSPACE_ROOT in .env, then:
cargo run --release

# One-shot
cargo run --release -- --workspace /path/to/your/repo "fix the failing test"
```

Resume a session:

```bash
cargo run --release -- --workspace /path/to/your/repo --session <id>
```

## First session (suggested)

1. `What’s in this repo? Give a short overview.`
2. `Find one small issue and propose a plan (don’t edit yet).` — or paste your own bug
3. `Execute the plan.` — on the first edit: **Enter** or `t` = approve for the rest of the task
4. `Show the diff and summarize.`

Useful slash commands: `/overview` `/intent plan` `/intent execute` `/plan` `/task` `/exit`

## Safety defaults

| Mode | Edit policy |
|------|-------------|
| Interactive | `confirm` unless `EDIT_POLICY` is set |
| One-shot | `auto` unless `EDIT_POLICY` is set |

Approvals: **Enter** / `t` = approve for task · `a` = once · `r` = reject. Shell commands that mutate the workspace (e.g. `sed -i`) are blocked — use `apply_patch`.

## Config

See [`agent-v1/.env.example`](agent-v1/.env.example). Important vars:

- `LLM_PROVIDER` / provider API key
- `WORKSPACE_ROOT` (or `--workspace`)
- `EDIT_POLICY` — `auto` \| `confirm` \| `readonly`
- `MAX_STEPS`

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `workspace not set` | Pass `--workspace` or set `WORKSPACE_ROOT` |
| `workspace does not exist` | Path must be an existing directory |
| Missing API key panic | Set the key for your `LLM_PROVIDER` in `.env` |
| `.env` ignored | Run from `agent-v1/` (or export vars yourself) |
| Edits denied in one-shot | Use interactive mode, or set `EDIT_POLICY=auto` |
