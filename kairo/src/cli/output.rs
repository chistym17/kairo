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
    /// True after the first assistant text in the current turn (avoids double blank lines).
    printed_assistant: bool,
    /// Last friendly error shown this turn (Finished can skip repeating it).
    last_error: Option<String>,
}

impl EventSink {
    pub fn new(format: OutputFormat) -> Self {
        Self {
            format,
            approved_for_task: false,
            printed_assistant: false,
            last_error: None,
        }
    }

    /// Call at the start of each user turn (interactive or oneshot).
    pub fn begin_turn(&mut self) {
        self.approved_for_task = false;
        self.printed_assistant = false;
        self.last_error = None;
    }

    pub fn is_jsonl(&self) -> bool {
        matches!(self.format, OutputFormat::Jsonl)
    }

    /// Banner / status for humans. JSONL stays silent on stdout (use stderr).
    pub fn banner(&self, mode: &str, model: &str, workspace: &Path, policy: EditPolicy) {
        let line = format!(
            "{} · {} · {} · edit={}",
            mode,
            model,
            workspace.display(),
            policy.as_str()
        );
        match self.format {
            OutputFormat::Text => println!("{line}"),
            OutputFormat::Jsonl => eprintln!("[kairo] {line}"),
        }
    }

    pub fn info(&self, message: &str) {
        match self.format {
            OutputFormat::Text => println!("{message}"),
            OutputFormat::Jsonl => eprintln!("[kairo] {message}"),
        }
    }

    /// Recoverable turn failure — print and let interactive mode continue.
    pub fn turn_failed(&mut self, err: &anyhow::Error) {
        let msg = humanize_error(&format!("{err:#}"));
        match self.format {
            OutputFormat::Text => {
                eprintln!();
                eprintln!("✗ {msg}");
                eprintln!("  You can try again, run `kairo config`, or type /exit.");
            }
            OutputFormat::Jsonl => {
                eprintln!("[kairo] turn failed: {msg}");
            }
        }
        self.last_error = Some(msg);
    }

    pub fn emit(&mut self, event: &AgentEvent) {
        match self.format {
            OutputFormat::Jsonl => {
                match serde_json::to_string(event) {
                    Ok(line) => println!("{line}"),
                    Err(e) => eprintln!("[kairo] failed to serialize event: {e}"),
                }
                let _ = io::stdout().flush();
            }
            OutputFormat::Text => self.print_human(event),
        }
    }

    /// Handle an approval request.
    pub async fn handle_approval(
        &mut self,
        session: &AgentSession,
        request: ApprovalRequest,
    ) -> anyhow::Result<()> {
        self.prompt_and_decide(session, &request).await
    }

    async fn prompt_and_decide(
        &mut self,
        session: &AgentSession,
        request: &ApprovalRequest,
    ) -> anyhow::Result<()> {
        if self.approved_for_task && matches!(request.kind, ApprovalKind::FileEdit) {
            session.approve(request.id.clone()).await?;
            return Ok(());
        }

        let decision = prompt_approval(request, self.is_jsonl())?;
        match decision {
            PromptDecision::ApproveForTask => {
                self.approved_for_task = true;
                session.approve_for_task(request.id.clone()).await?;
                self.info("Approved edits for this task.");
            }
            PromptDecision::ApproveOnce => {
                session.approve(request.id.clone()).await?;
            }
            PromptDecision::Reject => {
                session.reject(request.id.clone()).await?;
                self.info("Rejected.");
            }
        }
        Ok(())
    }

    fn print_human(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::Started { .. } => {
                // Quiet start — tools/reply carry the narrative.
            }
            AgentEvent::TextDelta { content } => {
                let t = content.trim();
                if t.is_empty() {
                    return;
                }
                if !self.printed_assistant {
                    println!();
                    self.printed_assistant = true;
                }
                println!("{t}");
            }
            AgentEvent::PlanUpdated { objective, steps, .. } => {
                println!();
                println!("Plan: {objective}");
                for step in steps.iter().take(8) {
                    println!("  {} {}", if step.done { "✓" } else { "·" }, step.text);
                }
                if steps.len() > 8 {
                    println!("  … {} more", steps.len() - 8);
                }
            }
            AgentEvent::ToolStarted { tool, summary, .. } => {
                println!("  · {}", tool_label(tool, summary));
            }
            AgentEvent::ToolCompleted {
                tool,
                success,
                summary,
                ..
            } => {
                if !*success {
                    println!("  ✗ {tool} failed — {}", shorten(summary, 120));
                }
            }
            AgentEvent::FileChanged { path } => {
                println!("  ✓ {}", path.display());
            }
            AgentEvent::VerificationCompleted {
                command,
                success,
                summary,
            } => {
                if *success {
                    println!("  ✓ verify {command}");
                } else {
                    println!("  ✗ verify {command} — {}", shorten(summary, 100));
                }
            }
            AgentEvent::ApprovalRequired { .. } => {
                // Prompted separately in handle_approval.
            }
            AgentEvent::Error { message } => {
                let friendly = humanize_error(message);
                // Avoid spamming identical errors within one turn.
                if self.last_error.as_deref() == Some(friendly.as_str()) {
                    return;
                }
                eprintln!();
                eprintln!("✗ {friendly}");
                self.last_error = Some(friendly);
            }
            AgentEvent::Finished { result } => self.print_finished(result),
        }
    }

    fn print_finished(&self, result: &RunResult) {
        println!();
        match result.status {
            RunStatus::Completed => {
                println!(
                    "Done · {} steps · {}ms{}",
                    result.steps,
                    result.duration_ms,
                    cost_suffix(result)
                );
            }
            RunStatus::CompletedUnverified => {
                println!(
                    "Done (unverified) · {} steps · {}ms{}",
                    result.steps,
                    result.duration_ms,
                    cost_suffix(result)
                );
            }
            RunStatus::Stopped => {
                let why = stopped_message(result, self.last_error.as_deref());
                eprintln!("Stopped — {why}");
                eprintln!("  You can try again or type /exit.");
            }
        }
        if let Some(summary) = &result.summary {
            let s = summary.trim();
            if !s.is_empty()
                && !matches!(result.status, RunStatus::Stopped)
                && self.last_error.as_deref() != Some(s)
            {
                println!("{s}");
            }
        }
        for path in &result.changed_files {
            println!("  touched {}", path.display());
        }
    }
}

fn cost_suffix(result: &RunResult) -> String {
    match result.usage.estimated_cost_usd {
        Some(cost) => format!(" · ${cost:.4}"),
        None => String::new(),
    }
}

fn stopped_message(result: &RunResult, last_error: Option<&str>) -> String {
    if let Some(err) = last_error {
        return err.to_string();
    }
    if let Some(summary) = &result.summary {
        let s = summary.trim();
        if !s.is_empty() {
            return humanize_error(s);
        }
    }
    humanize_error(&result.stop_reason)
}

pub(crate) fn user_facing_stop_message(result: &RunResult) -> String {
    stopped_message(result, None)
}

/// Map raw provider/runtime errors into short user-facing text.
pub fn humanize_error(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();

    if lower.contains("rate limit") || lower.contains("429") || lower.contains("too many requests")
    {
        return "Rate limited — wait a moment and try again.".into();
    }
    if lower.contains("401")
        || lower.contains("403")
        || lower.contains("unauthorized")
        || lower.contains("invalid api key")
        || lower.contains("incorrect api key")
        || lower.contains("authentication")
    {
        return "API key rejected — run `kairo config` and update the key.".into();
    }
    if lower.contains("404") && (lower.contains("model") || lower.contains("not found")) {
        return "Model not found — run `kairo config` and pick a different model.".into();
    }
    if lower.contains("timeout") || lower.contains("timed out") {
        return "Request timed out — try again.".into();
    }
    if lower.contains("connection")
        || lower.contains("dns")
        || lower.contains("network")
        || lower.contains("request failed")
    {
        return "Network error talking to the model — check connectivity and try again.".into();
    }
    if lower.contains("toolerror") || lower == "toolerror" {
        return "The model call failed.".into();
    }
    if lower.contains("maxstepsexceeded") {
        return "Hit the step limit before finishing.".into();
    }
    if lower.contains("cancelled") {
        return "Stopped.".into();
    }
    if lower.contains("noprogress") {
        return "No progress — try a clearer task.".into();
    }
    if lower.contains("modelgaveup") {
        return "The model stopped without finishing.".into();
    }

    shorten(raw, 200)
}

pub(crate) fn tool_label(tool: &str, summary: &str) -> String {
    let name = tool.trim();
    let detail = summary.trim();
    let verb = match name {
        "read_file" => "read",
        "write_file" => "write",
        "apply_patch" => "edit",
        "search_code" | "locate" | "grep" => "search",
        "run_shell" | "run_command" => "run",
        "git_status" | "git_diff" => "git",
        "list_dir" | "list_files" => "list",
        "finish" | "done" => "finish",
        other => other,
    };
    if detail.is_empty() || detail == name {
        verb.to_string()
    } else if detail.starts_with(name) {
        format!("{verb} {}", shorten(detail.trim_start_matches(name).trim(), 80))
    } else {
        format!("{verb} {}", shorten(detail, 80))
    }
}

fn shorten(s: &str, max: usize) -> String {
    let s = s.trim().replace('\n', " ");
    if s.chars().count() <= max {
        return s;
    }
    let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{truncated}…")
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
    writeln!(out, "Approval needed")?;
    writeln!(out, "  {}", request.summary)?;
    if !request.reason.is_empty() {
        writeln!(out, "  ({})", request.reason)?;
    }
    if !request.paths.is_empty() {
        let paths = request
            .paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(out, "  paths: {paths}")?;
    }
    if workspace_edit {
        writeln!(out, "  [t] approve for task  (recommended)")?;
        writeln!(out, "  [a] approve once")?;
        writeln!(out, "  [r] reject")?;
        write!(out, "Choice [t/a/r], Enter=t> ")?;
    } else {
        writeln!(out, "  [a] approve once")?;
        writeln!(out, "  [r] reject")?;
        write!(out, "Choice [a/r]> ")?;
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

pub fn exit_code(result: &RunResult) -> i32 {
    match result.status {
        RunStatus::Completed | RunStatus::CompletedUnverified => 0,
        RunStatus::Stopped => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanize_rate_limit() {
        assert!(humanize_error("API error: HTTP 429 Too Many Requests").contains("Rate limited"));
    }

    #[test]
    fn humanize_bad_key() {
        assert!(humanize_error("HTTP 401 Unauthorized").contains("API key"));
    }

    #[test]
    fn humanize_model_missing() {
        assert!(humanize_error("HTTP 404 model not found").contains("Model not found"));
    }

    #[test]
    fn tool_label_short() {
        assert_eq!(tool_label("read_file", "read_file"), "read");
        assert!(tool_label("apply_patch", "apply_patch src/a.rs").contains("edit"));
    }
}
