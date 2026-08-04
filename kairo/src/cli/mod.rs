//! Product CLI for `kairo`.
//!
//! Subcommands: `ask`, `agent`, `config`, `health`, `tui`. Bare `kairo` picks
//! TUI or the classic REPL from `KAIRO_UI` (default: tui) and auto-detects the
//! workspace from the nearest `.git` root.

mod args;
mod health;
mod output;
mod run;
mod workspace;

pub use args::{parse_args, CliArgs, Command, OutputFormat, SessionArg, UiMode};
pub use run::{main_from_env, run};
pub use workspace::{current_branch, detect_workspace, display_path};

pub fn print_usage() {
    eprintln!(
        "\
kairo — Kairo coding agent

Usage:
  kairo                              # KAIRO_UI (default: full-screen TUI)
  kairo ask [\"question\"]
  kairo agent [--edit-policy confirm|auto|readonly] [--task \"…\" | \"…\"]
  kairo tui                          # force full-screen TUI
  kairo tui --demo                   # layout preview without opening a session
  kairo config                       # view / edit provider, model, API key
  kairo health                       # check local readiness

UI (bare `kairo` only; subcommands always win):
  KAIRO_UI=tui                       # full-screen interface (default)
  KAIRO_UI=terminal                  # classic scrolling terminal
  Aliases: full/ui → tui; repl/cli/text → terminal

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
  -v, --verbose          Show harness debug lines ([agent], [prompt], …)
                         Also: KAIRO_VERBOSE=1
  -h, --help             Show this help

Logs:
  Numbered JSONL files under ./logs (e.g. kairo/logs/1_<id>.jsonl when you
  run from the crate). Override with KAIRO_LOG_DIR.
  Each turn prints [transition] / [state] and writes thinking_state records
  (always on — not quiet-gated).

Examples:
  cd ~/projects/shop && kairo
  kairo ask \"Where is auth handled?\"
  kairo agent --edit-policy auto --task \"Fix the failing tests\"
  kairo agent --verbose --task \"debug this\"
  kairo config
  kairo health
"
    );
}
