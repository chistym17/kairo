use serde::{Deserialize, Serialize};

/// Efficiency counters for one agent run.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RunEfficiency {
    pub total_steps: u32,
    pub total_model_turns: u32,
    pub parallel_tool_groups: u32,
    pub cache_hits: u32,
    pub cache_misses: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_to_first_edit_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_to_first_verification_ms: Option<u64>,
    pub duration_ms: u64,
    #[serde(default)]
    pub exploration_turns: u32,
    #[serde(default)]
    pub readonly_tools_in_exploration: u32,
    #[serde(default)]
    pub serial_batch_nudges: u32,
    #[serde(default)]
    pub no_edit_nudges: u32,
}

impl RunEfficiency {
    pub fn avg_readonly_per_exploration_turn(&self) -> Option<f64> {
        if self.exploration_turns == 0 {
            None
        } else {
            Some(self.readonly_tools_in_exploration as f64 / self.exploration_turns as f64)
        }
    }

    pub fn render_compact(&self) -> String {
        let mut parts = vec![
            format!("model_turns={}", self.total_model_turns),
            format!("tool_calls={}", self.total_steps),
            format!("parallel_groups={}", self.parallel_tool_groups),
            format!(
                "cache={}/{}",
                self.cache_hits,
                self.cache_hits + self.cache_misses
            ),
            format!("duration_ms={}", self.duration_ms),
        ];
        if let Some(avg) = self.avg_readonly_per_exploration_turn() {
            parts.push(format!("ro_per_explore={avg:.2}"));
        }
        if self.serial_batch_nudges > 0 {
            parts.push(format!("batch_nudges={}", self.serial_batch_nudges));
        }
        if self.no_edit_nudges > 0 {
            parts.push(format!("no_edit_nudges={}", self.no_edit_nudges));
        }
        if let Some(ms) = self.time_to_first_edit_ms {
            parts.push(format!("ttfe_ms={ms}"));
        }
        if let Some(ms) = self.time_to_first_verification_ms {
            parts.push(format!("ttfv_ms={ms}"));
        }
        parts.join(" ")
    }
}
