use serde::{Deserialize, Serialize};

/// Efficiency counters for one agent run (Phase 10.4).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
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
}

impl RunEfficiency {
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
        if let Some(ms) = self.time_to_first_edit_ms {
            parts.push(format!("ttfe_ms={ms}"));
        }
        if let Some(ms) = self.time_to_first_verification_ms {
            parts.push(format!("ttfv_ms={ms}"));
        }
        parts.join(" ")
    }
}
