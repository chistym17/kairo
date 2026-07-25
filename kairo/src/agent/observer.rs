use super::AgentEvent;

pub trait AgentObserver: Send + Sync {
    fn on_event(&self, event: AgentEvent);
}

