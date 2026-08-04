use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::session::Session;

pub fn default_sessions_dir() -> PathBuf {
    PathBuf::from("sessions")
}

pub fn session_dir(base: &Path, session_id: &str) -> PathBuf {
    base.join(session_id)
}

pub fn session_file_path(base: &Path, session_id: &str) -> PathBuf {
    session_dir(base, session_id).join("session.json")
}

pub fn save(base: &Path, session: &Session) -> Result<()> {
    let dir = session_dir(base, &session.id);
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create session directory {}", dir.display()))?;

    let path = session_file_path(base, &session.id);
    let json = serde_json::to_string_pretty(session).context("failed to serialize session")?;
    fs::write(&path, json).with_context(|| format!("failed to write {}", path.display()))?;

    Ok(())
}

pub fn load(base: &Path, session_id: &str) -> Result<Session> {
    let path = session_file_path(base, session_id);
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read session file {}", path.display()))?;
    let mut value: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse session file {}", path.display()))?;

    // Migrate Phase 12 Part 2 top-level `plan` into `task.plan`.
    if let Some(obj) = value.as_object_mut() {
        if let Some(plan) = obj.remove("plan") {
            let task = obj
                .entry("task")
                .or_insert_with(|| serde_json::json!({}));
            if let Some(task_obj) = task.as_object_mut() {
                task_obj.entry("plan").or_insert(plan);
            }
        }
        // Older sessions omitted schema_version — treat as v1.
        if obj.get("schema_version").is_none() {
            obj.insert("schema_version".into(), serde_json::json!(1));
        }
    }

    let mut session: Session = serde_json::from_value(value)
        .with_context(|| format!("failed to decode session file {}", path.display()))?;

    if session.schema_version > crate::session::SESSION_SCHEMA_VERSION {
        anyhow::bail!(
            "session {} schema_version {} is newer than supported {} — upgrade kairo",
            session_id,
            session.schema_version,
            crate::session::SESSION_SCHEMA_VERSION
        );
    }
    // Forward-fill so re-saves stamp the current version.
    session.schema_version = crate::session::SESSION_SCHEMA_VERSION;
    Ok(session)
}

pub fn exists(base: &Path, session_id: &str) -> bool {
    session_file_path(base, session_id).is_file()
}

/// Lightweight row for session browsers (does not keep full message history).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub workspace_root: PathBuf,
    pub updated_at: String,
    pub created_at: String,
    pub status: String,
    pub file_count: usize,
    pub model_id: String,
}

/// List persisted sessions, newest `updated_at` first. Skips corrupt entries.
pub fn list(base: &Path) -> Result<Vec<SessionSummary>> {
    if !base.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(base)
        .with_context(|| format!("failed to read sessions dir {}", base.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        if !exists(base, &id) {
            continue;
        }
        match load(base, &id) {
            Ok(session) => out.push(summary_from_session(&session)),
            Err(_) => continue,
        }
    }
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(out)
}

pub fn summary_from_session(session: &Session) -> SessionSummary {
    let title = session
        .task
        .objective
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            session.messages.iter().find_map(|m| match m {
                crate::session::Message::User { content } => {
                    let t = content.trim();
                    if t.is_empty() {
                        None
                    } else {
                        Some(truncate_title(t, 56))
                    }
                }
                _ => None,
            })
        })
        .unwrap_or_else(|| "New session".into());

    let file_count = session.task.modified_files.len();
    let verified = session.task.verification.iter().any(|v| {
        matches!(
            v.status,
            crate::verification::VerificationStatus::Passed
        )
    });
    let status = if verified {
        "Verified".into()
    } else if file_count > 0 {
        "Changed".into()
    } else if session.messages.is_empty() {
        "Empty".into()
    } else {
        "Saved".into()
    };

    SessionSummary {
        id: session.id.clone(),
        title,
        workspace_root: session.workspace_root.clone(),
        updated_at: session.updated_at.clone(),
        created_at: session.created_at.clone(),
        status,
        file_count,
        model_id: session.settings.model_id.clone(),
    }
}

fn truncate_title(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out.replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Message, SessionSettings};

    #[test]
    fn round_trip_session_json() {
        let base = tempfile::tempdir().unwrap();
        let mut session = Session::new("/tmp/project", SessionSettings::default());
        session.add_user_message("fix the bug");
        session.add_assistant_message("I'll inspect the code");
        session.task.plan = Some(
            crate::state::AgentPlan::from_update("Fix bug", "[ ] find\n[ ] fix").unwrap(),
        );

        save(base.path(), &session).unwrap();
        let loaded = load(base.path(), &session.id).unwrap();

        assert_eq!(loaded.id, session.id);
        assert_eq!(loaded.schema_version, crate::session::SESSION_SCHEMA_VERSION);
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0], Message::user("fix the bug"));
        let plan = loaded.task.plan.expect("plan persisted");
        assert_eq!(plan.objective, "Fix bug");
        assert_eq!(plan.version, 1);
        assert_eq!(plan.steps.len(), 2);
    }

    #[test]
    fn load_adds_schema_version_when_missing() {
        let base = tempfile::tempdir().unwrap();
        let session = Session::new("/tmp/project", SessionSettings::default());
        let dir = session_dir(base.path(), &session.id);
        fs::create_dir_all(&dir).unwrap();
        let path = session_file_path(base.path(), &session.id);
        // Write legacy JSON without schema_version.
        let mut v = serde_json::to_value(&session).unwrap();
        v.as_object_mut().unwrap().remove("schema_version");
        fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();

        let loaded = load(base.path(), &session.id).unwrap();
        assert_eq!(loaded.schema_version, crate::session::SESSION_SCHEMA_VERSION);
    }

    #[test]
    fn list_returns_newest_first() {
        let base = tempfile::tempdir().unwrap();
        let mut older = Session::new("/tmp/a", SessionSettings::default());
        older.add_user_message("old task");
        older.updated_at = "2020-01-01T00:00:00Z".into();
        save(base.path(), &older).unwrap();

        let mut newer = Session::new("/tmp/a", SessionSettings::default());
        newer.add_user_message("new task");
        newer.updated_at = "2024-06-01T00:00:00Z".into();
        save(base.path(), &newer).unwrap();

        let listed = list(base.path()).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, newer.id);
        assert!(listed[0].title.contains("new task"));
    }
}
