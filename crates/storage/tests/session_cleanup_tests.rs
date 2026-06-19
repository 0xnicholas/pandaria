use std::time::Duration;
use agent_core::{SessionEntry, SessionStore};
use storage::session::postgres::PgSessionStore;

async fn setup_pg() -> PgSessionStore {
    let db_url =
        std::env::var("PANDARIA_TEST_PG_URL").expect("PANDARIA_TEST_PG_URL not set");
    let pool = sqlx::PgPool::connect(&db_url).await.unwrap();
    let store = PgSessionStore::new(pool);
    store.init().await.unwrap();
    store
}

fn make_entry(text: &str) -> SessionEntry {
    SessionEntry::Message {
        id: uuid::Uuid::new_v4(),
        message: agent_core::AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: text.into(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }),
    }
}

#[tokio::test]
async fn test_cleanup_deletes_completed_sessions() {
    let store = setup_pg().await;
    store
        .save_session("t1", "s1", &[make_entry("hello")])
        .await
        .unwrap();
    store
        .update_session_status("t1", "s1", "completed")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let deleted = store
        .cleanup_expired_sessions(Duration::from_millis(50))
        .await
        .unwrap();
    assert_eq!(deleted, 1, "completed session should be cleaned up");
}

#[tokio::test]
async fn test_cleanup_preserves_active_sessions() {
    let store = setup_pg().await;
    store
        .save_session("t1", "s2", &[make_entry("active")])
        .await
        .unwrap();
    store
        .update_session_status("t1", "s2", "running")
        .await
        .unwrap();
    let deleted = store
        .cleanup_expired_sessions(Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(deleted, 0, "running session should NOT be cleaned up");
}
