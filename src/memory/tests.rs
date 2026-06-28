use super::SessionManager;
use crate::schema::Message;
use std::path::PathBuf;

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
