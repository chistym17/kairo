use std::path::PathBuf;

use crate::api::{AgentMode, EditPolicy};

use super::print_usage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Jsonl,
}

impl OutputFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "text" | "human" => Some(Self::Text),
            "jsonl" | "json" => Some(Self::Jsonl),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Jsonl => "jsonl",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Ask,
    Agent,
    /// Edit `~/.config/kairo/config.toml` interactively.
    Config,
    /// Check local configuration and workspace readiness.
    Health,
    /// Full-screen Ratatui shell (live agent events; `--demo` for static chrome).
    Tui,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionArg {
    /// Interactive default: create and persist.
    New,
    /// One-shot default: in-memory only.
    Ephemeral,
    Resume(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliArgs {
    pub command: Command,
    pub workspace: Option<PathBuf>,
    pub edit_policy: Option<EditPolicy>,
    pub output: OutputFormat,
    pub task: Option<String>,
    pub session: SessionArg,
    /// Show harness debug lines (`[agent]`, `[prompt]`, …).
    pub verbose: bool,
    /// `kairo tui --demo` — static layout shell without opening an agent.
    pub tui_demo: bool,
}

/// Parse `argv` (without the program name). Returns `None` when help was printed.
pub fn parse_args(args: &[String]) -> Result<Option<CliArgs>, String> {
    // Bare `kairo` → interactive agent; workspace is auto-detected at run time.
    if args.is_empty() {
        return Ok(Some(CliArgs {
            command: Command::Agent,
            workspace: None,
            edit_policy: None,
            output: OutputFormat::Text,
            task: None,
            session: SessionArg::New,
            verbose: false,
            tui_demo: false,
        }));
    }

    let first = args[0].as_str();
    if matches!(first, "-h" | "--help" | "help") {
        print_usage();
        return Ok(None);
    }

    let (command, rest) = match first {
        "ask" => (Command::Ask, &args[1..]),
        "agent" => (Command::Agent, &args[1..]),
        "config" => (Command::Config, &args[1..]),
        "health" => (Command::Health, &args[1..]),
        "tui" => (Command::Tui, &args[1..]),
        // Flags without a subcommand → agent (e.g. `kairo --edit-policy auto`).
        flag if flag.starts_with('-') => (Command::Agent, args),
        other => {
            return Err(format!(
                "unknown subcommand '{other}'. Use: ask | agent | config | health | tui\n\
                 Or run bare `kairo` for interactive agent mode.\n\
                 Try: kairo --help"
            ));
        }
    };

    if matches!(command, Command::Config | Command::Health) {
        if rest.iter().any(|a| matches!(a.as_str(), "-h" | "--help")) {
            print_usage();
            return Ok(None);
        }
        if !rest.is_empty() {
            let name = if matches!(command, Command::Config) {
                "config"
            } else {
                "health"
            };
            return Err(format!("{name} takes no arguments"));
        }
        return Ok(Some(CliArgs {
            command,
            workspace: None,
            edit_policy: None,
            output: OutputFormat::Text,
            task: None,
            session: SessionArg::Ephemeral,
            verbose: false,
            tui_demo: false,
        }));
    }

    if matches!(command, Command::Tui) {
        return parse_tui_args(rest);
    }

    let mut workspace: Option<PathBuf> = None;
    let mut edit_policy: Option<EditPolicy> = None;
    let mut output = OutputFormat::Text;
    let mut task: Option<String> = None;
    let mut session = SessionArg::Ephemeral;
    let mut session_explicit = false;
    let mut verbose = false;
    let mut positional: Vec<String> = Vec::new();

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "-h" | "--help" => {
                print_usage();
                return Ok(None);
            }
            "--workspace" => {
                let path = rest
                    .get(i + 1)
                    .ok_or_else(|| "--workspace requires a path".to_string())?;
                workspace = Some(PathBuf::from(path));
                i += 2;
            }
            "--edit-policy" => {
                let raw = rest
                    .get(i + 1)
                    .ok_or_else(|| "--edit-policy requires confirm|auto|readonly".to_string())?;
                edit_policy = Some(EditPolicy::parse(raw).ok_or_else(|| {
                    format!("invalid --edit-policy '{raw}' (use confirm|auto|readonly)")
                })?);
                i += 2;
            }
            "--output" => {
                let raw = rest
                    .get(i + 1)
                    .ok_or_else(|| "--output requires text|jsonl".to_string())?;
                output = OutputFormat::parse(raw)
                    .ok_or_else(|| format!("invalid --output '{raw}' (use text|jsonl)"))?;
                i += 2;
            }
            "--task" => {
                let text = rest
                    .get(i + 1)
                    .ok_or_else(|| "--task requires a string".to_string())?;
                task = Some(text.clone());
                i += 2;
            }
            "--session" => {
                let id = rest
                    .get(i + 1)
                    .ok_or_else(|| "--session requires an id".to_string())?;
                session = SessionArg::Resume(id.clone());
                session_explicit = true;
                i += 2;
            }
            "--new-session" => {
                session = SessionArg::New;
                session_explicit = true;
                i += 1;
            }
            "--verbose" | "-v" => {
                verbose = true;
                i += 1;
            }
            flag if flag.starts_with('-') => {
                return Err(format!("unknown flag {flag}"));
            }
            _ => {
                positional.push(rest[i].clone());
                i += 1;
            }
        }
    }

    if task.is_none() && !positional.is_empty() {
        task = Some(positional.join(" "));
    } else if task.is_some() && !positional.is_empty() {
        return Err("pass the task with --task or as trailing words, not both".into());
    }

    if matches!(command, Command::Ask) && edit_policy.is_some() {
        // Ask is always read-only; reject a contradictory flag early.
        if edit_policy != Some(EditPolicy::ReadOnly) {
            return Err(
                "ask mode is always read-only; omit --edit-policy (or pass readonly)".into(),
            );
        }
    }

    // Interactive sessions persist by default so /exit can resume later.
    if task.is_none() && !session_explicit {
        session = SessionArg::New;
    }

    Ok(Some(CliArgs {
        command,
        workspace,
        edit_policy,
        output,
        task,
        session,
        verbose,
        tui_demo: false,
    }))
}

fn parse_tui_args(rest: &[String]) -> Result<Option<CliArgs>, String> {
    let mut workspace: Option<PathBuf> = None;
    let mut edit_policy: Option<EditPolicy> = None;
    let mut verbose = false;
    let mut tui_demo = false;

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "-h" | "--help" => {
                print_usage();
                return Ok(None);
            }
            "--demo" => {
                tui_demo = true;
                i += 1;
            }
            "--workspace" => {
                let path = rest
                    .get(i + 1)
                    .ok_or_else(|| "--workspace requires a path".to_string())?;
                workspace = Some(PathBuf::from(path));
                i += 2;
            }
            "--edit-policy" => {
                let raw = rest
                    .get(i + 1)
                    .ok_or_else(|| "--edit-policy requires confirm|auto|readonly".to_string())?;
                edit_policy = Some(EditPolicy::parse(raw).ok_or_else(|| {
                    format!("invalid --edit-policy '{raw}' (use confirm|auto|readonly)")
                })?);
                i += 2;
            }
            "--verbose" | "-v" => {
                verbose = true;
                i += 1;
            }
            flag if flag.starts_with('-') => {
                return Err(format!("unknown tui flag {flag}"));
            }
            other => {
                return Err(format!("tui does not take positional arguments (got '{other}')"));
            }
        }
    }

    Ok(Some(CliArgs {
        command: Command::Tui,
        workspace,
        edit_policy,
        output: OutputFormat::Text,
        task: None,
        session: SessionArg::New,
        verbose,
        tui_demo,
    }))
}

impl CliArgs {
    pub fn mode(&self) -> AgentMode {
        match self.command {
            Command::Ask => AgentMode::Ask,
            Command::Agent | Command::Config | Command::Health | Command::Tui => AgentMode::Agent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn bare_invocation_is_interactive_agent() {
        let cli = parse_args(&[]).unwrap().unwrap();
        assert_eq!(cli.command, Command::Agent);
        assert!(cli.task.is_none());
        assert_eq!(cli.session, SessionArg::New);
        assert!(cli.workspace.is_none());
    }

    #[test]
    fn flags_without_subcommand_default_to_agent() {
        let cli = parse_args(&args(&["--edit-policy", "auto"]))
            .unwrap()
            .unwrap();
        assert_eq!(cli.command, Command::Agent);
        assert_eq!(cli.edit_policy, Some(EditPolicy::Auto));
    }

    #[test]
    fn ask_oneshot_positional() {
        let cli = parse_args(&args(&[
            "ask",
            "--workspace",
            "/tmp/repo",
            "where is auth?",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(cli.command, Command::Ask);
        assert_eq!(cli.task.as_deref(), Some("where is auth?"));
        assert_eq!(cli.session, SessionArg::Ephemeral);
        assert_eq!(cli.mode(), AgentMode::Ask);
    }

    #[test]
    fn agent_interactive_defaults_to_new_session() {
        let cli = parse_args(&args(&["agent", "--workspace", "/tmp/repo"]))
            .unwrap()
            .unwrap();
        assert!(cli.task.is_none());
        assert_eq!(cli.session, SessionArg::New);
        assert_eq!(cli.edit_policy, None);
        assert_eq!(cli.output, OutputFormat::Text);
    }

    #[test]
    fn agent_task_and_jsonl() {
        let cli = parse_args(&args(&[
            "agent",
            "--workspace",
            "/tmp/repo",
            "--edit-policy",
            "auto",
            "--output",
            "jsonl",
            "--task",
            "fix tests",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(cli.edit_policy, Some(EditPolicy::Auto));
        assert_eq!(cli.output, OutputFormat::Jsonl);
        assert_eq!(cli.task.as_deref(), Some("fix tests"));
        assert_eq!(cli.session, SessionArg::Ephemeral);
    }

    #[test]
    fn config_subcommand() {
        let cli = parse_args(&args(&["config"])).unwrap().unwrap();
        assert_eq!(cli.command, Command::Config);
    }

    #[test]
    fn config_rejects_extra_args() {
        let err = parse_args(&args(&["config", "show"])).unwrap_err();
        assert!(err.contains("no arguments"));
    }

    #[test]
    fn health_subcommand() {
        let cli = parse_args(&args(&["health"])).unwrap().unwrap();
        assert_eq!(cli.command, Command::Health);
    }

    #[test]
    fn tui_subcommand() {
        let cli = parse_args(&args(&["tui"])).unwrap().unwrap();
        assert_eq!(cli.command, Command::Tui);
        assert!(!cli.tui_demo);
    }

    #[test]
    fn tui_demo_flag() {
        let cli = parse_args(&args(&["tui", "--demo"])).unwrap().unwrap();
        assert_eq!(cli.command, Command::Tui);
        assert!(cli.tui_demo);
    }

    #[test]
    fn tui_rejects_unknown_flag() {
        let err = parse_args(&args(&["tui", "--task", "x"])).unwrap_err();
        assert!(err.contains("unknown tui flag") || err.contains("positional"));
    }

    #[test]
    fn ask_rejects_auto_edit_policy() {
        let err = parse_args(&args(&[
            "ask",
            "--workspace",
            "/tmp",
            "--edit-policy",
            "auto",
        ]))
        .unwrap_err();
        assert!(err.contains("read-only"));
    }

    #[test]
    fn unknown_subcommand_errors() {
        let err = parse_args(&args(&["run", "--workspace", "/tmp"])).unwrap_err();
        assert!(err.contains("unknown subcommand"));
    }

    #[test]
    fn help_returns_none() {
        assert!(parse_args(&args(&["--help"])).unwrap().is_none());
        assert!(parse_args(&args(&["agent", "--help"])).unwrap().is_none());
    }

    #[test]
    fn resume_session() {
        let cli = parse_args(&args(&[
            "agent",
            "--workspace",
            "/tmp",
            "--session",
            "abc",
            "continue",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(cli.session, SessionArg::Resume("abc".into()));
    }

    #[test]
    fn verbose_flag() {
        let cli = parse_args(&args(&["ask", "--verbose", "hello"]))
            .unwrap()
            .unwrap();
        assert!(cli.verbose);
        let cli = parse_args(&args(&["agent", "-v", "--task", "x"]))
            .unwrap()
            .unwrap();
        assert!(cli.verbose);
        assert!(!parse_args(&args(&["ask", "hi"])).unwrap().unwrap().verbose);
    }
}
