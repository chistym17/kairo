//! Welcome and session-browser screen state.

use std::path::PathBuf;

use chrono::{DateTime, Local, Utc};

use crate::session::SessionSummary;

#[derive(Debug, Clone)]
pub struct WelcomeState {
    pub workspace_display: String,
    pub provider: String,
    pub model: String,
    pub edit_policy: String,
    pub message: Option<String>,
}

impl WelcomeState {
    pub fn new(
        workspace_display: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        edit_policy: impl Into<String>,
    ) -> Self {
        Self {
            workspace_display: workspace_display.into(),
            provider: provider.into(),
            model: model.into(),
            edit_policy: edit_policy.into(),
            message: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionBrowserState {
    pub entries: Vec<SessionSummary>,
    pub selected: usize,
    pub scroll: u16,
    #[allow(dead_code)]
    pub filter_workspace: Option<PathBuf>,
}

impl SessionBrowserState {
    pub fn from_list(entries: Vec<SessionSummary>, filter_workspace: Option<PathBuf>) -> Self {
        let entries = if let Some(ws) = filter_workspace.as_ref() {
            entries
                .into_iter()
                .filter(|e| same_workspace(&e.workspace_root, ws))
                .collect()
        } else {
            entries
        };
        Self {
            entries,
            selected: 0,
            scroll: 0,
            filter_workspace,
        }
    }

    pub fn selected_id(&self) -> Option<&str> {
        self.entries.get(self.selected).map(|e| e.id.as_str())
    }

    pub fn move_sel(&mut self, delta: i32) {
        if self.entries.is_empty() {
            return;
        }
        let len = self.entries.len() as i32;
        let next = (self.selected as i32 + delta).rem_euclid(len);
        self.selected = next as usize;
    }
}

fn same_workspace(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(aa), Ok(bb)) => aa == bb,
        _ => a == b,
    }
}

#[derive(Debug, Clone)]
pub struct SessionGroup {
    pub label: String,
    pub indices: Vec<usize>,
}

/// Group session indices by calendar day relative to now (Today / Yesterday / Older).
pub fn group_sessions(entries: &[SessionSummary]) -> Vec<SessionGroup> {
    let now = Local::now().date_naive();
    let mut today = Vec::new();
    let mut yesterday = Vec::new();
    let mut older = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        let day = parse_day(&entry.updated_at);
        match day {
            Some(d) if d == now => today.push(i),
            Some(d) if d == now.pred_opt().unwrap_or(d) => yesterday.push(i),
            _ => older.push(i),
        }
    }

    let mut groups = Vec::new();
    if !today.is_empty() {
        groups.push(SessionGroup {
            label: "Today".into(),
            indices: today,
        });
    }
    if !yesterday.is_empty() {
        groups.push(SessionGroup {
            label: "Yesterday".into(),
            indices: yesterday,
        });
    }
    if !older.is_empty() {
        groups.push(SessionGroup {
            label: "Older".into(),
            indices: older,
        });
    }
    groups
}

fn parse_day(rfc3339: &str) -> Option<chrono::NaiveDate> {
    DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|dt| dt.with_timezone(&Local).date_naive())
        .or_else(|| {
            rfc3339
                .parse::<DateTime<Utc>>()
                .ok()
                .map(|dt| dt.with_timezone(&Local).date_naive())
        })
}

pub fn relative_time(rfc3339: &str) -> String {
    let Ok(dt) = DateTime::parse_from_rfc3339(rfc3339) else {
        return rfc3339.to_string();
    };
    let now = Utc::now();
    let then = dt.with_timezone(&Utc);
    let secs = now.signed_duration_since(then).num_seconds().max(0);
    if secs < 60 {
        "just now".into()
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 86_400 {
        format!("{} hour{} ago", secs / 3600, if secs / 3600 == 1 { "" } else { "s" })
    } else {
        format!("{} day{} ago", secs / 86_400, if secs / 86_400 == 1 { "" } else { "s" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn groups_today() {
        let now = Utc::now().to_rfc3339();
        let entries = vec![SessionSummary {
            id: "a".into(),
            title: "Fix bug".into(),
            workspace_root: PathBuf::from("/tmp"),
            updated_at: now,
            created_at: "x".into(),
            status: "Saved".into(),
            file_count: 1,
            model_id: "m".into(),
        }];
        let groups = group_sessions(&entries);
        assert_eq!(groups[0].label, "Today");
        assert_eq!(groups[0].indices, vec![0]);
    }
}
