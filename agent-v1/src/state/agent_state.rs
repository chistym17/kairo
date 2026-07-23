use std::path::PathBuf;
use std::time::Instant;
use uuid::Uuid;

use crate::authorization::AuthorizationState;
use crate::governance::GovernanceState;
use crate::repo_intel::HybridIntel;
use crate::session::{TaskSessionState, TurnIntent};
use crate::state::completion::CompletionTracker;
use crate::state::efficiency::RunEfficiency;
use crate::state::exploration::ExplorationTracker;
use crate::state::progress::ProgressState;
use crate::state::step::Step;
use crate::llm::{ModelPricing, UsageAccumulator};
use crate::tools::project_snapshot::ProjectSnapshot;
use crate::tools::read_cache::ReadCache;
use crate::tools::shell_cache::ShellCommandCache;

#[derive(Debug)]
pub struct AgentState {
    pub run_id: String,
    pub model_id: String,

    pub goal: String,
    pub workspace_root: PathBuf,
    pub history: Vec<Step>,
    pub step_count: u32,
    /// Number of LLM responses (a turn may include multiple tool calls).
    pub model_turns: u32,
    pub max_steps: u32,

    /// Edit policy + grants for this run (synced from session when present).
    pub authorization: AuthorizationState,

    /// Durable task memory for this run (synced from / to session).
    pub task: TaskSessionState,

    /// Declared turn intent for this run (metadata only — not a tool gate).
    pub turn_intent: TurnIntent,

    /// Orientation snapshot (computed once per run; refreshable via tool).
    pub project_snapshot: Option<ProjectSnapshot>,

    /// Hybrid repository index for `locate` (built once per run; updated on edit).
    pub repo_intel: Option<HybridIntel>,

    /// Successful read_file windows for this run (invalidated on edit).
    pub read_cache: ReadCache,

    /// Dedup / hard-cap for identical run_shell / run_command since last edit.
    pub shell_cache: ShellCommandCache,

    /// Serial exploration + soft no-edit budget tracking.
    pub exploration: ExplorationTracker,

    /// Consecutive no-progress / stagnation tracking (soft guidance).
    pub progress: ProgressState,

    /// Objective post-edit completion milestones.
    pub completion: CompletionTracker,

    /// Runtime governance — progress, deadlock detection, tool gating.
    pub governance: GovernanceState,

    /// Wall-clock start of this run (for duration / time-to-* metrics).
    pub run_started: Instant,
    /// Count of parallel read-only groups (size ≥ 2) executed.
    pub parallel_tool_groups: u32,
    pub ms_to_first_edit: Option<u64>,
    pub ms_to_first_verification: Option<u64>,

    /// Per-call model usage for this run (Phase 13A).
    pub model_usage: UsageAccumulator,
}

impl AgentState {
    pub fn new(
        goal: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
        model_id: impl Into<String>,
        max_steps: u32,
    ) -> Self {
        let model_id = model_id.into();
        Self {
            run_id: Uuid::new_v4().to_string(),
            model_id: model_id.clone(),
            goal: goal.into(),
            workspace_root: workspace_root.into(),
            history: Vec::new(),
            step_count: 0,
            model_turns: 0,
            max_steps,
            authorization: AuthorizationState::default(),
            task: TaskSessionState::default(),
            turn_intent: TurnIntent::Explore,
            project_snapshot: None,
            repo_intel: None,
            read_cache: ReadCache::new(),
            shell_cache: ShellCommandCache::new(),
            exploration: ExplorationTracker::new(),
            progress: ProgressState::new(),
            completion: CompletionTracker::new(),
            governance: GovernanceState::new(),
            run_started: Instant::now(),
            parallel_tool_groups: 0,
            ms_to_first_edit: None,
            ms_to_first_verification: None,
            model_usage: UsageAccumulator::new(model_id, ModelPricing::from_env()),
        }
    }

    pub fn is_exhausted(&self) -> bool {
        self.model_turns >= self.max_steps
    }

    pub fn record_step(&mut self, step: Step) {
        self.history.push(step);
        self.step_count += 1;
    }

    pub fn efficiency_snapshot(&self) -> RunEfficiency {
        RunEfficiency {
            total_steps: self.step_count,
            total_model_turns: self.model_turns,
            parallel_tool_groups: self.parallel_tool_groups,
            cache_hits: self.read_cache.hits,
            cache_misses: self.read_cache.misses,
            time_to_first_edit_ms: self.ms_to_first_edit,
            time_to_first_verification_ms: self.ms_to_first_verification,
            duration_ms: self.run_started.elapsed().as_millis() as u64,
            exploration_turns: self.exploration.exploration_turns,
            readonly_tools_in_exploration: self.exploration.readonly_tools_in_exploration,
            serial_batch_nudges: self.exploration.serial_batch_nudges,
            no_edit_nudges: self.exploration.no_edit_nudges,
            governance_rejections: self.governance.metrics.governance_rejections,
            repeated_actions_blocked: self.governance.metrics.repeated_actions_blocked,
            verification_families_blocked: self.governance.metrics.verification_families_blocked,
            review_or_finish_transitions: self.governance.metrics.review_or_finish_transitions,
            no_progress_terminations: self.governance.metrics.no_progress_terminations,
            turns_after_last_edit: self.governance.turns_since_edit,
            governance_mode: format!("{:?}", self.governance.mode),
        }
    }

    pub fn note_edit(&mut self, model_turn: u32) {
        if self.ms_to_first_edit.is_none() {
            self.ms_to_first_edit = Some(self.run_started.elapsed().as_millis() as u64);
        }
        self.shell_cache.clear();
        self.exploration.note_edit();
        self.completion.note_edit(model_turn);
        self.governance.note_edit(model_turn);
    }

    pub fn note_verification(&mut self) {
        if self.ms_to_first_verification.is_none() {
            self.ms_to_first_verification = Some(self.run_started.elapsed().as_millis() as u64);
        }
    }
}
