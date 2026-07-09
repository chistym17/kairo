use std::path::PathBuf;

use agent_v1::agent::AgentRuntime;
use agent_v1::config;
use agent_v1::context::default_system_prompt;
use agent_v1::llm::groq_client::GroqClient;
use agent_v1::session::{Session, SessionManager, SessionSettings};
use agent_v1::state::stop_reason::StopReason;

enum SessionArg {
    None,
    New,
    Load(String),
}

fn parse_args() -> (SessionArg, String) {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        print_usage();
        std::process::exit(1);
    }

    let mut session_arg = SessionArg::None;
    let mut goal_parts = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--new-session" => {
                session_arg = SessionArg::New;
                i += 1;
            }
            "--session" => {
                let id = args.get(i + 1).unwrap_or_else(|| {
                    eprintln!("error: --session requires an id");
                    std::process::exit(1);
                });
                session_arg = SessionArg::Load(id.clone());
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
        eprintln!("error: goal is required");
        print_usage();
        std::process::exit(1);
    }

    (session_arg, goal_parts.join(" "))
}

fn print_usage() {
    eprintln!("usage:");
    eprintln!("  agent-v1 \"<goal>\"");
    eprintln!("  agent-v1 --new-session \"<goal>\"");
    eprintln!("  agent-v1 --session <id> \"<goal>\"");
    eprintln!();
    eprintln!("example: agent-v1 \"fix the failing test in test_calc.py\"");
}

fn load_or_create_session(
    manager: &SessionManager,
    session_arg: SessionArg,
    workspace_root: &PathBuf,
    settings: SessionSettings,
) -> Option<Session> {
    match session_arg {
        SessionArg::None => None,
        SessionArg::New => {
            let session = manager
                .create(workspace_root.clone(), settings)
                .unwrap_or_else(|e| {
                    eprintln!("[agent-v1] failed to create session: {e:#}");
                    std::process::exit(1);
                });
            println!("[agent-v1] new session: {}", session.id);
            Some(session)
        }
        SessionArg::Load(id) => {
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

#[tokio::main]
async fn main() {
    dotenv::dotenv().ok();

    let (session_arg, goal) = parse_args();
    let config = config::Config::load();

    println!("[agent-v1] starting");
    println!("[agent-v1] goal: {}", goal);
    println!("[agent-v1] workspace: {}", config.workspace_root.display());
    println!("[agent-v1] model: {}", config.model_id);
    println!("[agent-v1] max_steps: {}", config.max_steps);
    println!("[agent-v1] step_delay: {}ms", config.step_delay_ms);

    if !config.workspace_root.exists() {
        eprintln!(
            "[agent-v1] error: workspace_root does not exist: {}",
            config.workspace_root.display()
        );
        std::process::exit(1);
    }

    let session_settings = SessionSettings {
        model_id: config.model_id.clone(),
        max_steps: config.max_steps,
    };

    let manager = SessionManager::with_default_dir();
    let mut session = load_or_create_session(
        &manager,
        session_arg,
        &config.workspace_root,
        session_settings,
    );

    let llm = GroqClient::new(&config.groq_api_key, &config.model_id);
    let runtime = AgentRuntime::new(
        Box::new(llm),
        &config.model_id,
        config.max_steps,
        config.step_delay_ms,
        config.tool_use_max_retries,
        config.tool_use_retry_delay_ms,
        default_system_prompt(),
    );

    let result = runtime
        .run(
            goal,
            config.workspace_root.clone(),
            None,
            session.as_mut(),
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
    println!("[agent-v1] total steps: {}", result.total_steps);
    println!("[agent-v1] log: {}", result.log_path.display());

    match result.stop_reason {
        StopReason::TaskComplete => std::process::exit(0),
        _ => std::process::exit(1),
    }
}
