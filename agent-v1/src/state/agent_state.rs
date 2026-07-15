use std::path::PathBuf;
use std::time::Instant;
use uuid::Uuid;

use crate::state::efficiency::RunEfficiency;
use crate::state::plan::AgentPlan;
use crate::state::step::Step;
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

    /// Optional working plan for multi-step tasks (Phase 7.3).
    pub plan: Option<AgentPlan>,

    /// Orientation snapshot (computed once per run; refreshable via tool).
    pub project_snapshot: Option<ProjectSnapshot>,

    /// Successful read_file windows for this run (invalidated on edit).
    pub read_cache: ReadCache,

    /// Dedup / hard-cap for identical run_shell / run_command since last edit.
    pub shell_cache: ShellCommandCache,

    /// Wall-clock start of this run (for duration / time-to-* metrics).
    pub run_started: Instant,
    /// Count of parallel read-only groups (size ≥ 2) executed.
    pub parallel_tool_groups: u32,
    pub ms_to_first_edit: Option<u64>,
    pub ms_to_first_verification: Option<u64>,
}

impl AgentState {
    pub fn new(
        goal: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
        model_id: impl Into<String>,
        max_steps: u32,
    ) -> Self {
        Self {
            run_id: Uuid::new_v4().to_string(),
            model_id: model_id.into(),
            goal: goal.into(),
            workspace_root: workspace_root.into(),
            history: Vec::new(),
            step_count: 0,
            model_turns: 0,
            max_steps,
            plan: None,
            project_snapshot: None,
            read_cache: ReadCache::new(),
            shell_cache: ShellCommandCache::new(),
            run_started: Instant::now(),
            parallel_tool_groups: 0,
            ms_to_first_edit: None,
            ms_to_first_verification: None,
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
        }
    }

    pub fn note_edit(&mut self) {
        if self.ms_to_first_edit.is_none() {
            self.ms_to_first_edit = Some(self.run_started.elapsed().as_millis() as u64);
        }
        // Verifies after an edit are meaningful again.
        self.shell_cache.clear();
    }

    pub fn note_verification(&mut self) {
        if self.ms_to_first_verification.is_none() {
            self.ms_to_first_verification = Some(self.run_started.elapsed().as_millis() as u64);
        }
    }
}
