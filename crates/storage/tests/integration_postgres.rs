use std::sync::Arc;

use agent_core::{
    CompactionActor, CompactionConfig, DefaultFileOperationExtractor,
    SessionActor, SessionEntry, SessionStore,
};
use agent_core::test_utils::{AllowAllDispatcher, TestProvider};
use storage::session::postgres::PgSessionStore;
use sqlx::PgPool;

/// Connect to the local PostgreSQL test database.
///
/// Uses `DATABASE_URL` env var if set, otherwise connects to
/// `postgres://$USER@localhost:5432/pandaria_test`.
async fn start_pg() -> PgPool {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        format!(
            "postgres://{}@localhost:5432/pandaria_test",
            std::env::var("USER").unwrap_or_else(|_| "postgres".to_string())
        )
    });
    PgPool::connect(&url)
        .await
        .expect("failed to connect to pandaria_test. set DATABASE_URL or run `createdb pandaria_test`")
}

/// Wipe the `sessions` table so each test starts clean.
async fn clean_table(pool: &PgPool) {
    sqlx::query("TRUNCATE TABLE sessions")
        .execute(pool)
        .await
        .expect("failed to truncate sessions table");
}

fn make_compaction_actor(provider: Arc<dyn ai_provider::LlmProvider>) -> Arc<CompactionActor> {
    Arc::new(CompactionActor::new(
        CompactionConfig::default(),
        provider,
        "test".to_string(),
        Arc::new(DefaultFileOperationExtractor::default()),
    ))
}

#[tokio::test]
async fn test_pg_store_roundtrip() {
    let _ = tracing_subscriber::fmt().try_init();
    let pool = start_pg().await;
    let store = PgSessionStore::new(pool.clone());
    store.init().await.expect("init failed");
    clean_table(&pool).await;

    let tenant = "roundtrip_t";
    let session = "roundtrip_s";
    let entries = vec![
        SessionEntry::Message {
            id: uuid::Uuid::new_v4(),
            message: agent_core::AgentMessage::User(ai_provider::UserMessage {
                content: vec![ai_provider::Content::Text {
                    text: "hello".to_string(),
                    text_signature: None,
                }],
                timestamp: std::time::SystemTime::now(),
            }),
        },
    ];

    store.save_session(tenant, session, &entries).await.expect("save failed");
    let loaded = store.load_session(tenant, session).await.expect("load failed");

    assert_eq!(loaded.len(), 1);
    match &loaded[0] {
        SessionEntry::Message { message, .. } => match message {
            agent_core::AgentMessage::User(u) => {
                assert!(u.content.iter().any(|c| matches!(c, ai_provider::Content::Text { text, .. } if text == "hello")));
            }
            _ => panic!("expected user message"),
        },
        _ => panic!("expected Message entry"),
    }
}

#[tokio::test]
async fn test_pg_store_overwrite() {
    let _ = tracing_subscriber::fmt().try_init();
    let pool = start_pg().await;
    let store = PgSessionStore::new(pool.clone());
    store.init().await.expect("init failed");
    clean_table(&pool).await;

    let tenant = "overwrite_t";
    let session = "overwrite_s";
    let entries_v1 = vec![SessionEntry::Message {
        id: uuid::Uuid::new_v4(),
        message: agent_core::AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: "first".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }),
    }];

    let entries_v2 = vec![
        SessionEntry::Message {
            id: uuid::Uuid::new_v4(),
            message: agent_core::AgentMessage::User(ai_provider::UserMessage {
                content: vec![ai_provider::Content::Text {
                    text: "first".to_string(),
                    text_signature: None,
                }],
                timestamp: std::time::SystemTime::now(),
            }),
        },
        SessionEntry::Message {
            id: uuid::Uuid::new_v4(),
            message: agent_core::AgentMessage::Assistant(ai_provider::AssistantMessage {
                content: vec![ai_provider::Content::Text {
                    text: "second".to_string(),
                    text_signature: None,
                }],
                provider: "test".to_string(),
                model: "test".to_string(),
                api: ai_provider::Api { provider: "test".to_string(), model: "test".to_string() },
                usage: ai_provider::Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    total_tokens: 2,
                },
                stop_reason: ai_provider::StopReason::Stop,
                response_id: None,
                error_message: None,
                timestamp: std::time::SystemTime::now(),
            }),
        },
    ];

    store.save_session(tenant, session, &entries_v1).await.expect("save v1 failed");
    store.save_session(tenant, session, &entries_v2).await.expect("save v2 failed");

    let loaded = store.load_session(tenant, session).await.expect("load failed");
    assert_eq!(loaded.len(), 2);
    match &loaded[1] {
        SessionEntry::Message { message, .. } => match message {
            agent_core::AgentMessage::Assistant(a) => {
                assert!(a.content.iter().any(|c| matches!(c, ai_provider::Content::Text { text, .. } if text == "second")));
            }
            _ => panic!("expected assistant message"),
        },
        _ => panic!("expected Message entry"),
    }
}

#[tokio::test]
async fn test_pg_store_tenant_isolation() {
    let _ = tracing_subscriber::fmt().try_init();
    let pool = start_pg().await;
    let store = PgSessionStore::new(pool.clone());
    store.init().await.expect("init failed");
    clean_table(&pool).await;

    let entries_a = vec![SessionEntry::Message {
        id: uuid::Uuid::new_v4(),
        message: agent_core::AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: "tenant_a".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }),
    }];

    let entries_b = vec![SessionEntry::Message {
        id: uuid::Uuid::new_v4(),
        message: agent_core::AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: "tenant_b".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }),
    }];

    store.save_session("tenant_a", "session_1", &entries_a)
        .await
        .expect("save a failed");
    store.save_session("tenant_b", "session_1", &entries_b)
        .await
        .expect("save b failed");

    let loaded_a = store.load_session("tenant_a", "session_1").await.expect("load a failed");
    let loaded_b = store.load_session("tenant_b", "session_1").await.expect("load b failed");

    assert_eq!(loaded_a.len(), 1);
    assert_eq!(loaded_b.len(), 1);

    match &loaded_a[0] {
        SessionEntry::Message { message, .. } => match message {
            agent_core::AgentMessage::User(u) => {
                assert!(u.content.iter().any(|c| matches!(c, ai_provider::Content::Text { text, .. } if text == "tenant_a")));
            }
            _ => panic!("expected user message"),
        },
        _ => panic!("expected Message entry"),
    }

    match &loaded_b[0] {
        SessionEntry::Message { message, .. } => match message {
            agent_core::AgentMessage::User(u) => {
                assert!(u.content.iter().any(|c| matches!(c, ai_provider::Content::Text { text, .. } if text == "tenant_b")));
            }
            _ => panic!("expected user message"),
        },
        _ => panic!("expected Message entry"),
    }
}

#[tokio::test]
async fn test_session_actor_persistence_loop() {
    let _ = tracing_subscriber::fmt().try_init();
    let pool = start_pg().await;
    let store = Arc::new(PgSessionStore::new(pool.clone()));
    store.init().await.expect("init failed");
    clean_table(&pool).await;

    let provider = TestProvider::text("pong");
    let dispatcher = Arc::new(AllowAllDispatcher);

    let tenant = "loop_t";
    let session = "loop_s";

    // 1. Create session, prompt, flush
    {
        let mut session_actor = SessionActor::new(
            tenant.to_string(),
            session.to_string(),
            "You are helpful.".to_string(),
            "echo".to_string(),
            provider.clone(),
            dispatcher.clone(),
            make_compaction_actor(provider.clone()),
            vec![],
            Some(store.clone()),
        );

        session_actor.prompt("ping".to_string()).await.expect("prompt failed");
        session_actor.flush().await.expect("flush failed");
    }

    // 2. Restore into new session actor
    let mut session2 = SessionActor::new(
        tenant.to_string(),
        session.to_string(),
        "You are helpful.".to_string(),
        "echo".to_string(),
        provider.clone(),
        dispatcher,
        make_compaction_actor(provider),
        vec![],
        Some(store.clone()),
    );

    let restored = session2.restore().await.expect("restore failed");
    assert!(restored > 0, "expected restored entries > 0");

    let msgs = session2.messages();
    assert!(msgs.len() >= 2, "expected at least user + assistant messages");

    // Verify the user message survived roundtrip
    assert!(msgs.iter().any(|m| {
        if let agent_core::AgentMessage::User(u) = m {
            u.content.iter().any(|c| matches!(c, ai_provider::Content::Text { text, .. } if text == "ping"))
        } else {
            false
        }
    }));
}

#[tokio::test]
async fn test_pg_store_delete() {
    let _ = tracing_subscriber::fmt().try_init();
    let pool = start_pg().await;
    let store = PgSessionStore::new(pool.clone());
    store.init().await.expect("init failed");
    clean_table(&pool).await;

    let tenant = "delete_t";
    let session = "delete_s";
    let entries = vec![SessionEntry::Message {
        id: uuid::Uuid::new_v4(),
        message: agent_core::AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: "to-delete".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }),
    }];

    store.save_session(tenant, session, &entries).await.expect("save failed");
    let loaded_before = store.load_session(tenant, session).await.expect("load before failed");
    assert_eq!(loaded_before.len(), 1);

    store.delete_session(tenant, session).await.expect("delete failed");
    let loaded_after = store.load_session(tenant, session).await.expect("load after failed");
    assert!(loaded_after.is_empty(), "expected session to be deleted");
}

#[tokio::test]
async fn test_pg_store_list() {
    let _ = tracing_subscriber::fmt().try_init();
    let pool = start_pg().await;
    let store = PgSessionStore::new(pool.clone());
    store.init().await.expect("init failed");
    clean_table(&pool).await;

    let tenant = "list_t";
    let entries = vec![SessionEntry::Message {
        id: uuid::Uuid::new_v4(),
        message: agent_core::AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: "list".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }),
    }];

    store.save_session(tenant, "s1", &entries).await.expect("save s1 failed");
    store.save_session(tenant, "s2", &entries).await.expect("save s2 failed");
    store.save_session(tenant, "s3", &entries).await.expect("save s3 failed");

    let mut sids = store.list_sessions(tenant).await.expect("list failed");
    sids.sort();
    assert_eq!(sids, vec!["s1", "s2", "s3"]);
}

#[tokio::test]
async fn test_pg_store_tenant_list_isolation() {
    let _ = tracing_subscriber::fmt().try_init();
    let pool = start_pg().await;
    let store = PgSessionStore::new(pool.clone());
    store.init().await.expect("init failed");
    clean_table(&pool).await;

    let entries = vec![SessionEntry::Message {
        id: uuid::Uuid::new_v4(),
        message: agent_core::AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: "iso".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }),
    }];

    store.save_session("tenant_x", "sx1", &entries).await.expect("save x failed");
    store.save_session("tenant_x", "sx2", &entries).await.expect("save x2 failed");
    store.save_session("tenant_y", "sy1", &entries).await.expect("save y failed");

    let list_x = store.list_sessions("tenant_x").await.expect("list x failed");
    let mut list_x_sorted = list_x.clone();
    list_x_sorted.sort();
    assert_eq!(list_x_sorted, vec!["sx1", "sx2"]);

    let list_y = store.list_sessions("tenant_y").await.expect("list y failed");
    assert_eq!(list_y, vec!["sy1"]);
}
