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
        println!("\n[approval] required");
        println!("[approval] effect:  {:?}", request.effect);
        println!("[approval] action:  {}", request.summary);
        println!("[approval] reason:  {}", request.reason);
        println!("[approval] options:");
        println!("  [a] approve once");
        if matches!(request.effect, ToolEffect::WorkspaceMutation) {
            println!(
                "  [t] approve for task (ordinary workspace mutations until objective change / revoke / session end)"
            );
        }
        println!("  [r] reject");
        print!("[approval] choice> ");
        let _ = io::stdout().flush();

        let mut line = String::new();
        if io::stdin().read_line(&mut line).is_err() {
            return ApprovalDecision::Reject;
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "a" | "approve" | "once" | "approve once" | "approve-once" => {
                ApprovalDecision::ApproveOnce
            }
            "t" | "task" | "approve for task" | "approve-task" | "approve_for_task"
                if matches!(request.effect, ToolEffect::WorkspaceMutation) =>
            {
                ApprovalDecision::ApproveForTask
            }
            _ => ApprovalDecision::Reject,
        }
    }
}
