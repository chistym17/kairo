//! Shared mid-run controls (Ask↔Agent) readable by the loop engine.

use std::sync::{Arc, Mutex};

use crate::authorization::EditPolicy;

/// Edit policy + system prompt the loop re-reads each model/tool step.
#[derive(Debug, Clone)]
pub struct LiveControls {
    /// True while the public API mode is Ask (read-only Q&A).
    pub is_ask: bool,
    pub edit_policy: EditPolicy,
    pub system_prompt: String,
}

impl LiveControls {
    pub fn new(
        is_ask: bool,
        edit_policy: EditPolicy,
        system_prompt: impl Into<String>,
    ) -> Self {
        Self {
            is_ask,
            edit_policy,
            system_prompt: system_prompt.into(),
        }
    }

    pub fn shared(
        is_ask: bool,
        edit_policy: EditPolicy,
        system_prompt: impl Into<String>,
    ) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::new(is_ask, edit_policy, system_prompt)))
    }
}
