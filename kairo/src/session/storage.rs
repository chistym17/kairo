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
}
