use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::agent::AgentEvent as InternalEvent;
use crate::agent::RunResult as InternalRunResult;
use crate::authorization::{
    ApprovalRequest as InternalApprovalRequest, Scope, ToolEffect,
};
use crate::state::stop_reason::StopReason;

/// Everything a caller (CLI, web UI, script) needs to follow a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Started {
        run_id: String,
        mode: String,
        workspace: PathBuf,
        model: String,
    },
    /// Assistant prose. Emitted per model turn, not per token.
    TextDelta {
        content: String,
    },
    PlanUpdated {
        version: u32,
        objective: String,
        steps: Vec<PlanStep>,
    },
    ToolStarted {
        id: String,
        tool: String,
        summary: String,
    },
    ToolCompleted {
        id: String,
        tool: String,
        success: bool,
        summary: String,
    },
    FileChanged {
        path: PathBuf,
    },
    VerificationCompleted {
        command: String,
        success: bool,
        summary: String,
    },
    /// The run is paused until [`super::AgentSession::approve`] or
    /// [`super::AgentSession::reject`] is called with this request's id.
    ApprovalRequired {
        request: ApprovalRequest,
    },
    Error {
        message: String,
    },
    Finished {
        result: RunResult,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub text: String,
    pub done: bool,
}

/// A pending decision the agent cannot make for the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub kind: ApprovalKind,
    /// Action awaiting approval, e.g. `apply_patch src/store.js`.
    pub summary: String,
    pub reason: String,
    /// Paths the action would touch, when known.
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    /// Ordinary workspace edit. [`super::AgentSession::approve_for_task`] covers later edits.
    FileEdit,
    Destructive,
    /// Installs, network writes, and other effects outside the workspace.
    ExternalEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Finished with a green verification.
    Completed,
    /// Finished, but nothing verified the result.
    CompletedUnverified,
    /// Stopped without finishing — see [`RunResult::stop_reason`].
    Stopped,
}

/// Outcome of one `send`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunResult {
    pub run_id: String,
    pub status: RunStatus,
    pub success: bool,
    pub summary: Option<String>,
    pub changed_files: Vec<PathBuf>,
    pub steps: u32,
    pub model_turns: u32,
    pub duration_ms: u64,
    /// Internal stop reason, for diagnostics and logs.
    pub stop_reason: String,
    pub log_path: PathBuf,
    pub usage: RunUsage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunUsage {
    pub model_calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub estimated_cost_usd: Option<f64>,
}

impl RunResult {
    /// Result for a turn that never produced a run (e.g. the runtime refused to start).
    pub(crate) fn stopped(reason: impl Into<String>) -> Self {
        Self {
            run_id: String::new(),
            status: RunStatus::Stopped,
            success: false,
            summary: None,
            changed_files: Vec::new(),
            steps: 0,
            model_turns: 0,
            duration_ms: 0,
            stop_reason: reason.into(),
            log_path: PathBuf::new(),
            usage: RunUsage::default(),
        }
    }
}

/// Translate an internal observer event into the public stream.
///
/// Returns `None` for internal bookkeeping the public API does not promise
/// (step boundaries, run start/finish — those are emitted by the session).
pub(crate) fn map_event(event: InternalEvent) -> Option<AgentEvent> {
    match event {
        InternalEvent::ToolStarted {
            step_index, tool, ..
        } => Some(AgentEvent::ToolStarted {
            id: step_index.to_string(),
            tool: tool.clone(),
            summary: tool,
        }),
        InternalEvent::ToolCompleted {
            step_index,
            tool,
            success,
            ..
        } => Some(AgentEvent::ToolCompleted {
            id: step_index.to_string(),
            summary: format!("{tool} {}", if success { "ok" } else { "failed" }),
            tool,
            success,
        }),
        InternalEvent::FileModified { path, .. } => Some(AgentEvent::FileChanged {
            path: PathBuf::from(path),
        }),
        InternalEvent::ModelText { text, .. } => Some(AgentEvent::TextDelta { content: text }),
        InternalEvent::PlanUpdated {
            version,
            objective,
            steps,
            ..
        } => Some(AgentEvent::PlanUpdated {
            version,
            objective,
            steps: steps
                .into_iter()
                .map(|s| PlanStep {
                    text: s.text,
                    done: s.done,
                })
                .collect(),
        }),
        InternalEvent::VerificationCompleted {
            command,
            success,
            summary,
            ..
        } => Some(AgentEvent::VerificationCompleted {
            command,
            success,
            summary,
        }),
        InternalEvent::Error { message, .. } => Some(AgentEvent::Error { message }),
        // Shell output is reported through ToolCompleted; only discovered checks
        // become VerificationCompleted.
        InternalEvent::CommandCompleted { .. }
        | InternalEvent::StepStarted { .. }
        | InternalEvent::StepCompleted { .. }
        | InternalEvent::RunStarted { .. }
        | InternalEvent::RunFinished { .. } => None,
    }
}

pub(crate) fn map_approval_request(request: &InternalApprovalRequest) -> ApprovalRequest {
    ApprovalRequest {
        id: request.id.0.clone(),
        kind: match request.effect {
            ToolEffect::DestructiveMutation => ApprovalKind::Destructive,
            ToolEffect::ExternalSideEffect => ApprovalKind::ExternalEffect,
            // ReadOnly never reaches a gate; treat it as an edit if it ever does.
            ToolEffect::WorkspaceMutation | ToolEffect::ReadOnly => ApprovalKind::FileEdit,
        },
        summary: request.summary.clone(),
        reason: request.reason.clone(),
        paths: match &request.proposed_scope {
            Scope::Paths(paths) => paths.clone(),
            Scope::Unspecified => Vec::new(),
        },
    }
}

pub(crate) fn map_run_result(result: InternalRunResult) -> RunResult {
    let status = match result.stop_reason {
        StopReason::TaskComplete => RunStatus::Completed,
        StopReason::CompletedUnverified => RunStatus::CompletedUnverified,
        _ => RunStatus::Stopped,
    };
    RunResult {
        run_id: result.run_id,
        status,
        success: result.success,
        summary: result.summary,
        changed_files: result.changed_files.into_iter().map(PathBuf::from).collect(),
        steps: result.total_steps,
        model_turns: result.total_model_turns,
        duration_ms: result.efficiency.duration_ms,
        stop_reason: format!("{:?}", result.stop_reason),
        log_path: result.log_path,
        usage: RunUsage {
            model_calls: result.usage.calls,
            input_tokens: result.usage.input_tokens,
            output_tokens: result.usage.output_tokens,
            cached_input_tokens: result.usage.cached_input_tokens,
            estimated_cost_usd: result.usage.estimated_total_cost_usd,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authorization::{ApprovalId, ToolEffect};

    #[test]
    fn tool_events_carry_a_stable_id() {
        let started = map_event(InternalEvent::ToolStarted {
            run_id: "r".into(),
            step_index: 3,
            tool: "apply_patch".into(),
        })
        .unwrap();
        let completed = map_event(InternalEvent::ToolCompleted {
            run_id: "r".into(),
            step_index: 3,
            tool: "apply_patch".into(),
            success: false,
        })
        .unwrap();

        match (started, completed) {
            (
                AgentEvent::ToolStarted { id: a, .. },
                AgentEvent::ToolCompleted {
                    id: b,
                    success,
                    summary,
                    ..
                },
            ) => {
                assert_eq!(a, b);
                assert!(!success);
                assert!(summary.contains("failed"));
            }
            other => panic!("unexpected mapping: {other:?}"),
        }
    }

    #[test]
    fn internal_bookkeeping_is_not_public() {
        assert!(map_event(InternalEvent::StepStarted {
            run_id: "r".into(),
            step_index: 0
        })
        .is_none());
        assert!(map_event(InternalEvent::RunFinished {
            run_id: "r".into(),
            stop_reason: "TaskComplete".into(),
            total_steps: 1,
            success: true
        })
        .is_none());
    }

    #[test]
    fn plan_and_verification_map_to_public_shapes() {
        let plan = map_event(InternalEvent::PlanUpdated {
            run_id: "r".into(),
            version: 2,
            objective: "fix tests".into(),
            steps: vec![crate::state::plan::PlanStep {
                text: "patch store.js".into(),
                done: true,
            }],
        })
        .unwrap();
        match plan {
            AgentEvent::PlanUpdated {
                version,
                objective,
                steps,
            } => {
                assert_eq!(version, 2);
                assert_eq!(objective, "fix tests");
                assert_eq!(steps, vec![PlanStep { text: "patch store.js".into(), done: true }]);
            }
            other => panic!("unexpected mapping: {other:?}"),
        }

        let verify = map_event(InternalEvent::VerificationCompleted {
            run_id: "r".into(),
            command: "npm test".into(),
            success: true,
            summary: "12 passed".into(),
        })
        .unwrap();
        assert!(matches!(
            verify,
            AgentEvent::VerificationCompleted { success: true, .. }
        ));
    }

    #[test]
    fn approval_request_exposes_paths_and_kind() {
        let internal = InternalApprovalRequest {
            id: ApprovalId("abc".into()),
            effect: ToolEffect::WorkspaceMutation,
            summary: "apply_patch src/store.js".into(),
            proposed_scope: Scope::Paths(vec![PathBuf::from("src/store.js")]),
            reason: "needs approval".into(),
            expires_at_revision: None,
        };
        let public = map_approval_request(&internal);
        assert_eq!(public.id, "abc");
        assert_eq!(public.kind, ApprovalKind::FileEdit);
        assert_eq!(public.paths, vec![PathBuf::from("src/store.js")]);

        let external = InternalApprovalRequest {
            effect: ToolEffect::ExternalSideEffect,
            proposed_scope: Scope::Unspecified,
            ..internal
        };
        let public = map_approval_request(&external);
        assert_eq!(public.kind, ApprovalKind::ExternalEffect);
        assert!(public.paths.is_empty());
    }
}
