# agent-v1

Coding-agent binary for this repo.

Full install / run docs: **[../README.md](../README.md)**

```bash
cp .env.example .env   # set API key (+ optional WORKSPACE_ROOT)

# Ask (read-only)
cargo run --release -- ask --workspace /path/to/repo "Where is auth handled?"

# Agent (edits; default edit policy = confirm)
cargo run --release -- agent --workspace /path/to/repo
cargo run --release -- agent --workspace /path/to/repo --edit-policy auto --task "Fix the failing tests"

# Machine-readable events (one JSON object per line on stdout)
cargo run --release -- ask --workspace /path/to/repo --output jsonl "…"

# Web UI (Milestone 4 — stub for now)
cargo run --release -- serve --workspace /path/to/repo
```

Library embedding: see `examples/basic_agent.rs` and `agent_v1::api`.
