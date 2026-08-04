use std::path::{Path, PathBuf};

use anyhow::Result;

use super::session::{Session, SessionSettings};
use super::storage::{self, default_sessions_dir, SessionSummary};

/// Creates, loads, and saves sessions on disk.
pub struct SessionManager {
    sessions_dir: PathBuf,
}

impl SessionManager {
    pub fn new(sessions_dir: impl Into<PathBuf>) -> Self {
        Self {
            sessions_dir: sessions_dir.into(),
        }
    }

    pub fn with_default_dir() -> Self {
        Self::new(default_sessions_dir())
    }

    pub fn sessions_dir(&self) -> &Path {
        &self.sessions_dir
    }

    pub fn create(
        &self,
        workspace_root: impl Into<PathBuf>,
        settings: SessionSettings,
    ) -> Result<Session> {
        let session = Session::new(workspace_root, settings);
        self.save(&session)?;
        Ok(session)
    }

    pub fn load(&self, session_id: &str) -> Result<Session> {
        storage::load(&self.sessions_dir, session_id)
    }

    pub fn save(&self, session: &Session) -> Result<Session> {
        storage::save(&self.sessions_dir, session)?;
        Ok(session.clone())
    }

    pub fn exists(&self, session_id: &str) -> bool {
        storage::exists(&self.sessions_dir, session_id)
    }

    /// Newest-first summaries for the session browser.
    pub fn list(&self) -> Result<Vec<SessionSummary>> {
        storage::list(&self.sessions_dir)
    }

    pub fn update<F>(&self, session_id: &str, mut f: F) -> Result<Session>
    where
        F: FnMut(&mut Session),
    {
        let mut session = self.load(session_id)?;
        f(&mut session);
        self.save(&session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Message;

    #[test]
    fn create_load_and_continue_session() {
        let base = tempfile::tempdir().unwrap();
        let manager = SessionManager::new(base.path());

        let session = manager
            .create("./workspace", SessionSettings::default())
            .unwrap();
        let id = session.id.clone();

        manager
            .update(&id, |s| s.add_user_message("fix divide bug"))
            .unwrap();

        let loaded = manager.load(&id).unwrap();
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.messages[0], Message::user("fix divide bug"));

        manager
            .update(&id, |s| s.add_user_message("now add tests"))
            .unwrap();

        let continued = manager.load(&id).unwrap();
        assert_eq!(continued.messages.len(), 2);
        assert_eq!(continued.messages[1], Message::user("now add tests"));
    }
}
