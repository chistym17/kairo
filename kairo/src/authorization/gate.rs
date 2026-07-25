use std::io::{self, Write};

use super::types::{ApprovalDecision, ApprovalRequest, ToolEffect};

/// Interactive (or test) source of approval decisions.
pub trait ApprovalGate: Send + Sync {
    fn request_approval(&self, request: &ApprovalRequest) -> ApprovalDecision;
}

/// Always approve once — for tests that need a gate without prompting.
pub struct AutoApprovalGate;

impl ApprovalGate for AutoApprovalGate {
    fn request_approval(&self, _request: &ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::ApproveOnce
    }
}

/// Prompt on stdin for interactive sessions.
pub struct StdinApprovalGate;

impl ApprovalGate for StdinApprovalGate {
    fn request_approval(&self, request: &ApprovalRequest) -> ApprovalDecision {
        let workspace = matches!(request.effect, ToolEffect::WorkspaceMutation);
        println!("\n[approval] required");
        println!("[approval] effect:  {:?}", request.effect);
        println!("[approval] action:  {}", request.summary);
        println!("[approval] reason:  {}", request.reason);
        println!("[approval] options:");
        if workspace {
            println!(
                "  [t] approve for task  (recommended — covers later apply_patch/write_file this task)"
            );
            println!("  [a] approve once");
        } else {
            println!("  [a] approve once");
        }
        println!("  [r] reject");
        if workspace {
            print!("[approval] choice [t/a/r], Enter=t> ");
        } else {
            print!("[approval] choice> ");
        }
        let _ = io::stdout().flush();

        let mut line = String::new();
        if io::stdin().read_line(&mut line).is_err() {
            return ApprovalDecision::Reject;
        }
        let trimmed = line.trim().to_ascii_lowercase();
        // Empty Enter defaults to ApproveForTask for workspace mutations.
        if trimmed.is_empty() && workspace {
            return ApprovalDecision::ApproveForTask;
        }
        match trimmed.as_str() {
            "a" | "approve" | "once" | "approve once" | "approve-once" => {
                ApprovalDecision::ApproveOnce
            }
            "t" | "task" | "approve for task" | "approve-task" | "approve_for_task"
                if workspace =>
            {
                ApprovalDecision::ApproveForTask
            }
            _ => ApprovalDecision::Reject,
        }
    }
}
