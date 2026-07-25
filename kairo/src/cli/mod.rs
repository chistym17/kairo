//! Product CLI for `kairo`.
//!
//! Subcommands: `ask`, `agent`, `serve`, `config`, `health`. Bare `kairo` starts
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
kairo — Kairo coding agent

Usage:
  kairo                              # interactive agent in this repo
  kairo ask [\"question\"]
  kairo agent [--edit-policy confirm|auto|readonly] [--task \"…\" | \"…\"]
  kairo serve [--port 8787]
  kairo config                       # view / edit provider, model, API key
  kairo health                       # check local readiness

Workspace (auto-detected; --workspace optional):
  1. --workspace <path>
  2. nearest parent directory containing .git
  3. current directory

Provider config (~/.config/kairo/config.toml):
  First run prompts for provider, API key, and model.
  `kairo config` edits the same file later.
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
  cd ~/projects/shop && kairo
  kairo ask \"Where is auth handled?\"
  kairo agent --edit-policy auto --task \"Fix the failing tests\"
  kairo serve --port 8787
  kairo config
  kairo health
"
    );
}
