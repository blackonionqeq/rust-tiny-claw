use super::Session;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
pub struct SessionManager {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_create(
        &self,
        id: impl Into<String>,
        work_dir: impl Into<PathBuf>,
    ) -> Arc<Session> {
        let id = id.into();
        let mut sessions = self.sessions.lock().expect("session manager lock poisoned");
        // Session identity is stable: if the id already exists, keep its original
        // workspace binding instead of silently moving an active conversation.
        sessions
            .entry(id.clone())
            .or_insert_with(|| Arc::new(Session::new(id, work_dir)))
            .clone()
    }

    pub fn len(&self) -> usize {
        self.sessions
            .lock()
            .expect("session manager lock poisoned")
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
