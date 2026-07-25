# agent-v1

Coding-agent package (CLI + library + localhost UI).

Root onboarding (install, keys, five-minute path): **[../README.md](../README.md)**

```bash
cp agent-v1/.env.example agent-v1/.env   # set LLM_PROVIDER + API key

# From any git repo — workspace is auto-detected
cd /path/to/your/repo
cargo run -p agent-v1 --release --              # interactive agent
cargo run -p agent-v1 --release -- ask "Where is auth?"
cargo run -p agent-v1 --release -- serve --port 8787

# After install
agent-v1
agent-v1 serve
```

Library embedding: `examples/basic_agent.rs` and `agent_v1::api`.
