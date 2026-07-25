# agent-v1

Coding-agent package (CLI + library + localhost UI).

Root onboarding (install, keys, five-minute path): **[../README.md](../README.md)**

```bash
# from repo root
cp agent-v1/.env.example agent-v1/.env   # set LLM_PROVIDER + API key

cargo run -p agent-v1 --release -- ask --workspace /path/to/repo "Where is auth?"
cargo run -p agent-v1 --release -- agent --workspace /path/to/repo
cargo run -p agent-v1 --release -- serve --workspace /path/to/repo --port 8787

# after cargo install --path agent-v1  (or --git … --bin agent-v1)
agent-v1 serve --workspace /path/to/repo
```

Library embedding: `examples/basic_agent.rs` and `agent_v1::api`.
