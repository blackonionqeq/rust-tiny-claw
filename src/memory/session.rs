use crate::schema::Message;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::SystemTime;

#[derive(Debug)]
pub struct Session {
    id: String,
    work_dir: PathBuf,
    created_at: SystemTime,
    updated_at: RwLock<SystemTime>,
    // Full per-session transcript. Provider calls should use working_memory()
    // instead of reading this directly, so long chats do not grow every request.
    history: RwLock<Vec<Message>>,
    // A session is the ordering boundary for a conversation. Different sessions
    // may run concurrently, but two runs for the same session must not interleave
    // user input, assistant actions, and tool observations.
    run_lock: Mutex<()>,
}

impl Session {
    pub fn new(id: impl Into<String>, work_dir: impl Into<PathBuf>) -> Self {
        let now = SystemTime::now();
        Self {
            id: id.into(),
            work_dir: work_dir.into(),
            created_at: now,
            updated_at: RwLock::new(now),
            history: RwLock::new(Vec::new()),
            run_lock: Mutex::new(()),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    pub fn created_at(&self) -> SystemTime {
        self.created_at
    }

    pub fn updated_at(&self) -> SystemTime {
        *self
            .updated_at
            .read()
            .expect("session updated timestamp lock poisoned")
    }

    pub fn append(&self, message: Message) {
        self.append_many([message]);
    }

    pub fn append_many(&self, messages: impl IntoIterator<Item = Message>) {
        let mut history = self.history.write().expect("session history lock poisoned");
        history.extend(messages);
        *self
            .updated_at
            .write()
            .expect("session updated timestamp lock poisoned") = SystemTime::now();
    }

    pub fn history(&self) -> Vec<Message> {
        self.history
            .read()
            .expect("session history lock poisoned")
            .clone()
    }

    pub fn working_memory(&self, limit: usize) -> Vec<Message> {
        let history = self.history.read().expect("session history lock poisoned");
        if limit == 0 || history.len() <= limit {
            return history.clone();
        }

        let mut memory = history[history.len() - limit..].to_vec();
        // Tool observations are only valid when the matching assistant tool call
        // is still in context. Drop leading observations created by the slice
        // boundary to avoid sending orphaned tool results to provider APIs.
        while memory
            .first()
            .is_some_and(|message| message.tool_call_id.is_some())
        {
            memory.remove(0);
        }
        memory
    }

    pub fn lock_run(&self) -> std::sync::MutexGuard<'_, ()> {
        self.run_lock.lock().expect("session run lock poisoned")
    }
}
