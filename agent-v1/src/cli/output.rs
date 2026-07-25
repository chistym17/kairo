use std::io::{self, Write};
use std::path::Path;

use crate::api::{
    AgentEvent, AgentSession, ApprovalKind, ApprovalRequest, EditPolicy, RunResult, RunStatus,
};

use super::args::OutputFormat;

/// How the CLI presents events and answers approvals.
pub struct EventSink {
    format: OutputFormat,
    /// After the first file-edit ApproveForTask, later FileEdit approvals auto-approve once.
    approved_for_task: bool,
}

impl EventSink {
    pub fn new(format: OutputFormat) -> Self {
        Self {
            format,
            approved_for_task: false,
        }
    }

    pub fn is_jsonl(&self) -> bool {
        matches!(self.format, OutputFormat::Jsonl)
    }

    /// Banner / status for humans. JSONL stays silent on stdout (use stderr).
    pub fn banner(&self, mode: &str, model: &str, workspace: &Path, policy: EditPolicy) {
        let line = format!(
            "[agent-v1] {} · {} · {} · edit={}",
            mode,
            model,
            workspace.display(),
            policy.as_str()
        );
        match self.format {
            OutputFormat::Text => println!("{line}"),
            OutputFormat::Jsonl => eprintln!("{line}"),
        }
    }

    pub fn info(&self, message: &str) {
        match self.format {
            OutputFormat::Text => println!("[agent-v1] {message}"),
            OutputFormat::Jsonl => eprintln!("[agent-v1] {message}"),
        }
    }

    pub fn emit(&self, event: &AgentEvent) {
        match self.format {
            OutputFormat::Jsonl => {
                match serde_json::to_string(event) {
                    Ok(line) => println!("{line}"),
                    Err(e) => eprintln!("[agent-v1] failed to serialize event: {e}"),
                }
                let _ = io::stdout().flush();
            }
            OutputFormat::Text => print_human(event),
        }
    }

    /// Handle an approval request. Returns true if the turn should continue.
    pub async fn handle_approval(
        &mut self,
        session: &AgentSession,
        request: ApprovalRequest,
    ) -> anyhow::Result<()> {
        match self.format {
            OutputFormat::Jsonl => {
                // Event already emitted; still need a decision from a human on stderr/stdin.
                self.prompt_and_decide(session, &request).await
            }
            OutputFormat::Text => self.prompt_and_decide(session, &request).await,
        }
    }

    async fn prompt_and_decide(
        &mut self,
        session: &AgentSession,
        request: &ApprovalRequest,
    ) -> anyhow::Result<()> {
        // Already granted for this task — approve once without re-prompting.
        if self.approved_for_task && matches!(request.kind, ApprovalKind::FileEdit) {
            session.approve(request.id.clone()).await?;
            return Ok(());
        }

        let decision = prompt_approval(request, self.is_jsonl())?;
        match decision {
            PromptDecision::ApproveForTask => {
                self.approved_for_task = true;
                session.approve_for_task(request.id.clone()).await?;
                self.info("approved edits for this task");
            }
            PromptDecision::ApproveOnce => {
                session.approve(request.id.clone()).await?;
            }
            PromptDecision::Reject => {
                session.reject(request.id.clone()).await?;
                self.info("rejected");
            }
        }
        Ok(())
    }
}

enum PromptDecision {
    ApproveOnce,
    ApproveForTask,
    Reject,
}

fn prompt_approval(request: &ApprovalRequest, jsonl: bool) -> anyhow::Result<PromptDecision> {
    let out: &mut dyn Write = if jsonl {
        &mut io::stderr()
    } else {
        &mut io::stdout()
    };

    let workspace_edit = matches!(request.kind, ApprovalKind::FileEdit);
    writeln!(out)?;
    writeln!(out, "[approval] required")?;
    writeln!(out, "[approval] kind:    {:?}", request.kind)?;
    writeln!(out, "[approval] action:  {}", request.summary)?;
    writeln!(out, "[approval] reason:  {}", request.reason)?;
    if !request.paths.is_empty() {
        let paths = request
            .paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(out, "[approval] paths:   {paths}")?;
    }
    writeln!(out, "[approval] options:")?;
    if workspace_edit {
        writeln!(
            out,
            "  [t] approve for task  (recommended — later edits this task)"
        )?;
        writeln!(out, "  [a] approve once")?;
    } else {
        writeln!(out, "  [a] approve once")?;
    }
    writeln!(out, "  [r] reject")?;
    if workspace_edit {
        write!(out, "[approval] choice [t/a/r], Enter=t> ")?;
    } else {
        write!(out, "[approval] choice> ")?;
    }
    out.flush()?;

    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return Ok(PromptDecision::Reject);
    }
    let trimmed = line.trim().to_ascii_lowercase();
    if trimmed.is_empty() && workspace_edit {
        return Ok(PromptDecision::ApproveForTask);
    }
    Ok(match trimmed.as_str() {
        "a" | "approve" | "once" | "approve once" | "approve-once" => PromptDecision::ApproveOnce,
        "t" | "task" | "approve for task" | "approve-task" | "approve_for_task"
            if workspace_edit =>
        {
            PromptDecision::ApproveForTask
        }
        _ => PromptDecision::Reject,
    })
}

fn print_human(event: &AgentEvent) {
    match event {
        AgentEvent::Started { run_id, mode, .. } => {
            println!("[{mode}] run {run_id}");
        }
        AgentEvent::TextDelta { content } => {
            let t = content.trim();
            if !t.is_empty() {
                println!("agent: {t}");
            }
        }
        AgentEvent::PlanUpdated {
            version,
            objective,
            steps,
        } => {
            println!("plan v{version}: {objective}");
            for step in steps {
                println!("  [{}] {}", if step.done { "x" } else { " " }, step.text);
            }
        }
        AgentEvent::ToolStarted { tool, .. } => println!("→ {tool}"),
        AgentEvent::ToolCompleted { tool, success, .. } => {
            println!("← {tool} {}", if *success { "ok" } else { "failed" });
        }
        AgentEvent::FileChanged { path } => println!("changed {}", path.display()),
        AgentEvent::VerificationCompleted {
            command,
            success,
            summary,
        } => {
            println!(
                "verify {command}: {} — {summary}",
                if *success { "green" } else { "red" }
            );
        }
        AgentEvent::ApprovalRequired { .. } => {
            // Prompted separately in handle_approval.
        }
        AgentEvent::Error { message } => eprintln!("error: {message}"),
        AgentEvent::Finished { result } => print_finished(result),
    }
}

fn print_finished(result: &RunResult) {
    println!();
    println!(
        "[agent-v1] {:?} · {} steps · {} turns · {}ms",
        result.status, result.steps, result.model_turns, result.duration_ms
    );
    if let Some(summary) = &result.summary {
        println!("[agent-v1] summary: {summary}");
    }
    for path in &result.changed_files {
        println!("[agent-v1] touched: {}", path.display());
    }
    if let Some(cost) = result.usage.estimated_cost_usd {
        println!("[agent-v1] cost: ${cost:.4}");
    }
    if !result.log_path.as_os_str().is_empty() {
        println!("[agent-v1] log: {}", result.log_path.display());
    }
}

pub fn exit_code(result: &RunResult) -> i32 {
    match result.status {
        RunStatus::Completed | RunStatus::CompletedUnverified => 0,
        RunStatus::Stopped => 1,
    }
}
