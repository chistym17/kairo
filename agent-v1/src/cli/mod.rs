//! Product CLI for `agent-v1`.
//!
//! Subcommands: `ask`, `agent`, `serve`, `config`, `health`. Bare `agent-v1` starts
//! interactive agent mode and auto-detects the workspace from the nearest `.git` root.

mod args;
mod health;
mod output;
mod run;
mod workspace;

pub use args::{parse_args, CliArgs, Command, OutputFormat, SessionArg};
pub use run::{main_from_env, run};
pub use workspace::{current_branch, detect_workspace, display_path};

pub fn print_usage() {
    eprintln!(
        "\
agent-v1 — local coding agent

Usage:
  agent-v1                              # interactive agent in this repo
  agent-v1 ask [\"question\"]
  agent-v1 agent [--edit-policy confirm|auto|readonly] [--task \"…\" | \"…\"]
  agent-v1 serve [--port 8787]
  agent-v1 config                       # view / edit provider, model, API key
  agent-v1 health                       # check local readiness

Workspace (auto-detected; --workspace optional):
  1. --workspace <path>
  2. nearest parent directory containing .git
  3. current directory

Provider config (~/.config/agent-v1/config.toml):
  First run prompts for provider, API key, and model.
  `agent-v1 config` edits the same file later.
  Env vars (LLM_PROVIDER, *_API_KEY, MODEL_ID) override the file.

Options:
  --workspace <path>     Override auto-detected repo root
  --edit-policy <p>      Agent mode: confirm (default) | auto | readonly
  --output <fmt>         text (default) | jsonl  (machine-readable events)
  --task <text>          One-shot task (otherwise interactive)
  --session <id>         Resume a persisted session
  --new-session          Persist a new session (default for interactive)
  --port <n>             serve only — listen on 127.0.0.1:<n> (default 8787)
  -h, --help             Show this help

Examples:
  cd ~/projects/shop && agent-v1
  agent-v1 ask \"Where is auth handled?\"
  agent-v1 agent --edit-policy auto --task \"Fix the failing tests\"
  agent-v1 serve --port 8787
  agent-v1 config
  agent-v1 health
"
    );
}
