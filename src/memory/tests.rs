use super::{Session, SessionManager};
use crate::schema::Message;
use std::path::PathBuf;

#[test]
fn working_memory_returns_recent_messages() {
    let session = Session::new("s1", ".");
    session.append_many([
        Message::user("one"),
        Message::assistant("two"),
        Message::user("three"),
    ]);

    let memory = session.working_memory(2);

    assert_eq!(memory.len(), 2);
    assert_eq!(memory[0].content, "two");
    assert_eq!(memory[1].content, "three");
}

#[test]
fn working_memory_drops_orphaned_observation_at_boundary() {
    let session = Session::new("s1", ".");
    session.append_many([
        Message::assistant("tool call was before the window"),
        Message::observation("call_1", "orphaned"),
        Message::user("next prompt"),
        Message::assistant("next answer"),
    ]);

    let memory = session.working_memory(3);

    assert_eq!(memory.len(), 2);
    assert_eq!(memory[0].content, "next prompt");
    assert_eq!(memory[1].content, "next answer");
}

#[test]
fn session_manager_reuses_existing_session() {
    let manager = SessionManager::new();

    let first = manager.get_or_create("chat_1", PathBuf::from("/workspace"));
    let second = manager.get_or_create("chat_1", PathBuf::from("/other"));

    assert_eq!(manager.len(), 1);
    assert!(std::sync::Arc::ptr_eq(&first, &second));
    assert_eq!(second.work_dir(), PathBuf::from("/workspace").as_path());
}

#[test]
fn session_manager_keeps_histories_isolated_by_id() {
    let manager = SessionManager::new();
    let front = manager.get_or_create("chat_front", PathBuf::from("/workspace"));
    let back = manager.get_or_create("chat_back", PathBuf::from("/workspace"));

    front.append(Message::user("front-only request"));
    back.append(Message::user("back-only request"));

    let front_history = front.history();
    let back_history = back.history();

    assert_eq!(front_history.len(), 1);
    assert_eq!(front_history[0].content, "front-only request");
    assert_eq!(back_history.len(), 1);
    assert_eq!(back_history[0].content, "back-only request");
}
