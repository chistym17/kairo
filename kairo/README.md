# Kairo

Coding-agent package (CLI + library + localhost UI) for [kairo.so](https://kairo.so).

Root onboarding (install, keys, five-minute path): **[../README.md](../README.md)**

```bash
cp kairo/.env.example kairo/.env   # set LLM_PROVIDER + API key

# From any git repo — workspace is auto-detected
cd /path/to/your/repo
cargo run -p kairo --release --              # interactive agent
cargo run -p kairo --release -- ask "Where is auth?"
cargo run -p kairo --release -- serve --port 8787

# After install
kairo
kairo serve
```

Library embedding: `examples/basic_agent.rs` and `kairo::api`.
