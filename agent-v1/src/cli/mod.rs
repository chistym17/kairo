//! Product CLI for `agent-v1`.
//!
//! Subcommands: `ask`, `agent`, `serve` (stub until Milestone 4).

mod args;
mod output;
mod run;

pub use args::{parse_args, CliArgs, Command, OutputFormat, SessionArg};
pub use run::{main_from_env, run};

pub fn print_usage() {
    eprintln!(
        "\
agent-v1 — local coding agent

Usage:
  agent-v1 ask   --workspace <path> [--session <id>] [\"question\"]
  agent-v1 agent --workspace <path> [--edit-policy confirm|auto|readonly]
                 [--session <id>] [--output text|jsonl] [--task \"…\" | \"…\"]
  agent-v1 serve --workspace <path>   # stub until web UI (Milestone 4)

Options:
  --workspace <path>     Repository root (or WORKSPACE_ROOT in .env)
  --edit-policy <p>      Agent mode only: confirm (default) | auto | readonly
  --output <fmt>         text (default) | jsonl  (machine-readable events)
  --task <text>          One-shot task (otherwise interactive)
  --session <id>         Resume a persisted session
  --new-session          Persist a new session (default for interactive)
  -h, --help             Show this help

Examples:
  agent-v1 ask --workspace ./my-app \"Where is auth handled?\"
  agent-v1 agent --workspace ./my-app
  agent-v1 agent --workspace ./my-app --edit-policy auto --task \"Fix the failing tests\"
  agent-v1 agent --workspace ./my-app --task \"Fix tests\" --output jsonl
"
    );
}
