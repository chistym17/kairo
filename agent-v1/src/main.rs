use std::path::PathBuf;

use agent_v1::agent::AgentRuntime;
use agent_v1::authorization::EditPolicy;
use agent_v1::chat::{open_session, ChatRuntime, SessionArg};
use agent_v1::config;
use agent_v1::context::default_system_prompt;
use agent_v1::llm::build_client;
use agent_v1::session::{Session, SessionManager, SessionSettings};
use agent_v1::state::stop_reason::StopReason;

enum RunMode {
    Interactive,
    OneShot { goal: String },
}

enum OneShotSessionArg {
    None,
    New,
    Load(String),
}

struct CliArgs {
    mode: RunMode,
    session_arg: OneShotSessionArg,
}

fn parse_args() -> CliArgs {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        return CliArgs {
            mode: RunMode::Interactive,
            session_arg: OneShotSessionArg::New,
        };
    }

    let mut session_arg = OneShotSessionArg::None;
    let mut goal_parts = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--new-session" => {
                session_arg = OneShotSessionArg::New;
                i += 1;
            }
            "--session" => {
                let id = args.get(i + 1).unwrap_or_else(|| {
                    eprintln!("error: --session requires an id");
                    std::process::exit(1);
                });
                session_arg = OneShotSessionArg::Load(id.clone());
                i += 2;
            }
            flag if flag.starts_with('-') => {
                eprintln!("error: unknown flag {}", flag);
                print_usage();
                std::process::exit(1);
            }
            _ => {
                goal_parts.push(args[i].clone());
                i += 1;
            }
        }
    }

    if goal_parts.is_empty() {
        let chat_session = match session_arg {
            OneShotSessionArg::None => OneShotSessionArg::New,
            other => other,
        };
        return CliArgs {
            mode: RunMode::Interactive,
            session_arg: chat_session,
        };
    }

    CliArgs {
        mode: RunMode::OneShot {
            goal: goal_parts.join(" "),
        },
        session_arg,
    }
}

fn print_usage() {
    eprintln!("usage:");
    eprintln!("  agent-v1                              # interactive session");
    eprintln!("  agent-v1 --session <id>               # resume interactive session");
    eprintln!("  agent-v1 \"<goal>\"                     # one-shot run");
    eprintln!("  agent-v1 --new-session \"<goal>\"");
    eprintln!("  agent-v1 --session <id> \"<goal>\"");
    eprintln!();
    eprintln!("interactive: type tasks at >, /exit to quit");
    eprintln!("cmds: /overview /intent <explore|plan|execute|review|clear> /plan /task");
    eprintln!("example: agent-v1 \"fix the failing test in test_calc.py\"");
}

fn load_oneshot_session(
    manager: &SessionManager,
    session_arg: OneShotSessionArg,
    workspace_root: &PathBuf,
    settings: SessionSettings,
) -> Option<Session> {
    match session_arg {
        OneShotSessionArg::None => None,
        OneShotSessionArg::New => {
            let session = manager
                .create(workspace_root.clone(), settings)
                .unwrap_or_else(|e| {
                    eprintln!("[agent-v1] failed to create session: {e:#}");
                    std::process::exit(1);
                });
            println!("[agent-v1] new session: {}", session.id);
            Some(session)
        }
        OneShotSessionArg::Load(id) => {
            let session = manager.load(&id).unwrap_or_else(|e| {
                eprintln!("[agent-v1] failed to load session {id}: {e:#}");
                std::process::exit(1);
            });
            if session.workspace_root != *workspace_root {
                eprintln!(
                    "[agent-v1] warning: session workspace ({}) differs from config ({})",
                    session.workspace_root.display(),
                    workspace_root.display()
                );
            }
            println!(
                "[agent-v1] resumed session: {} ({} prior messages)",
                session.id,
                session.messages.len()
            );
            Some(session)
        }
    }
}

fn to_chat_session_arg(arg: OneShotSessionArg) -> SessionArg {
    match arg {
        OneShotSessionArg::New => SessionArg::New,
        OneShotSessionArg::Load(id) => SessionArg::Load(id),
        OneShotSessionArg::None => SessionArg::New,
    }
}

fn build_runtime(config: &config::Config) -> AgentRuntime {
    let llm = build_client(config);
    AgentRuntime::new(
        llm,
        &config.model_id,
        config.max_steps,
        config.step_delay_ms,
        config.tool_use_max_retries,
        config.tool_use_retry_delay_ms,
        default_system_prompt(),
        config.max_context_tokens,
        config.context_reserve_tokens,
        config.keep_recent_session_messages,
    )
    .with_semantic_policy(config.semantic_policy)
}

#[tokio::main]
async fn main() {
    dotenv::dotenv().ok();

    let cli = parse_args();
    let config = config::Config::load();

    if !config.workspace_root.exists() {
        eprintln!(
            "[agent-v1] error: workspace_root does not exist: {}",
            config.workspace_root.display()
        );
        std::process::exit(1);
    }

    println!("[agent-v1] starting");
    println!("[agent-v1] provider: {}", config.llm_provider);
    println!("[agent-v1] workspace: {}", config.workspace_root.display());
    println!("[agent-v1] model: {}", config.model_id);
    println!("[agent-v1] max_steps: {}", config.max_steps);
    println!(
        "[agent-v1] semantic_policy: {}",
        config.semantic_policy
    );
    println!("[agent-v1] edit_policy: {:?}", config.edit_policy);
    if config.step_delay_ms > 0 {
        println!("[agent-v1] step_delay: {}ms", config.step_delay_ms);
    }

    // Interactive defaults to ConfirmBeforeFirstEdit unless EDIT_POLICY was set
    // explicitly to something other than the one-shot default of auto.
    let interactive_edit_policy = match std::env::var("EDIT_POLICY") {
        Ok(_) => config.edit_policy,
        Err(_) => EditPolicy::ConfirmBeforeFirstEdit,
    };

    let session_settings = SessionSettings {
        model_id: config.model_id.clone(),
        max_steps: config.max_steps,
        edit_policy: match cli.mode {
            RunMode::Interactive => interactive_edit_policy,
            RunMode::OneShot { .. } => config.edit_policy,
        },
    };

    let manager = SessionManager::with_default_dir();
    let runtime = build_runtime(&config);

    match cli.mode {
        RunMode::Interactive => {
            let session = open_session(
                &manager,
                to_chat_session_arg(cli.session_arg),
                &config.workspace_root,
                session_settings,
            )
            .unwrap_or_else(|e| {
                eprintln!("[agent-v1] failed to open session: {e:#}");
                std::process::exit(1);
            });

            let mut chat = ChatRuntime::new(
                manager,
                session,
                runtime,
                config.workspace_root.clone(),
            );

            chat.run_loop().await.unwrap_or_else(|e| {
                eprintln!("[agent-v1] chat error: {e:#}");
                std::process::exit(1);
            });
        }

        RunMode::OneShot { goal } => {
            println!("[agent-v1] goal: {}", goal);

            let mut session = load_oneshot_session(
                &manager,
                cli.session_arg,
                &config.workspace_root,
                session_settings,
            );

            let result = runtime
                .run(
                    goal,
                    config.workspace_root.clone(),
                    None,
                    session.as_mut(),
                    // One-shot: no stdin gate. Use EDIT_POLICY=auto (default) or
                    // confirm will deny mutations without a gate.
                    None,
                )
                .await
                .unwrap_or_else(|e| {
                    eprintln!("[agent-v1] runtime error: {e:#}");
                    std::process::exit(1);
                });

            if let Some(session) = session {
                manager.save(&session).unwrap_or_else(|e| {
                    eprintln!("[agent-v1] failed to save session: {e:#}");
                    std::process::exit(1);
                });
                println!("[agent-v1] session saved: {}", session.id);
            }

            println!("\n[agent-v1] ── run complete ──────────────────────────");
            println!("[agent-v1] run_id: {}", result.run_id);
            println!("[agent-v1] stop reason: {:?}", result.stop_reason);
            println!("[agent-v1] {}", result.efficiency.render_compact());
            if let Some(summary) = &result.summary {
                println!("[agent-v1] summary: {summary}");
            }
            println!("[agent-v1] log: {}", result.log_path.display());

            match result.stop_reason {
                StopReason::TaskComplete | StopReason::CompletedUnverified => {
                    std::process::exit(0)
                }
                _ => std::process::exit(1),
            }
        }
    }
}
