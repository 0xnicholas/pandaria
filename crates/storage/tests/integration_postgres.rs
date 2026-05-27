//! PostgreSQL session store integration tests.
//!
//! # Running with a local PostgreSQL instance
//!
//! If Docker is unavailable, you can run these tests against a local PostgreSQL
//! instance by setting the `PANDARIA_TEST_PG_URL` environment variable:
//!
//! ```bash
//! # Start local PostgreSQL (e.g. via Postgres.app or Homebrew)
//! pg_ctl -D "$HOME/Library/Application Support/Postgres/var-18" start
//!
//! # Run tests (must use --test-threads=1 because all tests share one DB)
//! PANDARIA_TEST_PG_URL="postgres://postgres@localhost:5432/postgres" \
//!   cargo test -p storage --test integration_postgres -- --test-threads=1
//! ```

use std::sync::Arc;

use agent_core::test_utils::{AllowAllDispatcher, TestProvider};
use agent_core::{
    Compactor, CompactionConfig, DefaultFileOperationExtractor, SessionActor, SessionConfig,
    SessionEntry, SessionStore,
};
use sqlx::PgPool;
use storage::session::postgres::PgSessionStore;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;

/// Start a PostgreSQL container and return a connection pool.
///
/// If `PANDARIA_TEST_PG_URL` is set, connects to the local PostgreSQL instance
/// instead of starting a Docker container via testcontainers. When using a local
/// DB, tests **must** run with `--test-threads=1` because all tests share the
/// same database and concurrent `CREATE TABLE IF NOT EXISTS` calls race.
async fn start_pg() -> (
    PgPool,
    Option<testcontainers_modules::testcontainers::ContainerAsync<Postgres>>,
) {
    if let Ok(url) = std::env::var("PANDARIA_TEST_PG_URL") {
        let pool = PgPool::connect(&url)
            .await
            .expect("failed to connect to local postgres (check PANDARIA_TEST_PG_URL)");
        return (pool, None);
    }

    let container = Postgres::default()
        .start()
        .await
        .expect("failed to start postgres container");
    let host = container
        .get_host()
        .await
        .expect("failed to get container host");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("failed to get container port");
    let url = format!("postgres://postgres:postgres@{}:{}/postgres", host, port);

    let pool = PgPool::connect(&url)
        .await
        .expect("failed to connect to testcontainers postgres");

    (pool, Some(container))
}

fn make_compaction_actor(provider: Arc<dyn ai_provider::LlmProvider>) -> Arc<Compactor> {
    Arc::new(Compactor::new(
        CompactionConfig::default(),
        provider,
        "test".to_string(),
        Arc::new(DefaultFileOperationExtractor::default()),
    ))
}

#[tokio::test]
async fn test_pg_store_roundtrip() {
    let _ = tracing_subscriber::fmt().try_init();
    let (pool, _container) = start_pg().await;
    let store = PgSessionStore::new(pool);
    store.init().await.expect("init failed");

    let tenant = "roundtrip_t";
    let session = "roundtrip_s";
    let entries = vec![SessionEntry::Message {
        id: uuid::Uuid::new_v4(),
        message: agent_core::AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: "hello".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }),
    }];

    store
        .save_session(tenant, session, &entries)
        .await
        .expect("save failed");
    let loaded = store
        .load_session(tenant, session)
        .await
        .expect("load failed");

    assert_eq!(loaded.len(), 1);
    match &loaded[0] {
        SessionEntry::Message { message, .. } => match message {
            agent_core::AgentMessage::User(u) => {
                assert!(u.content.iter().any(
                    |c| matches!(c, ai_provider::Content::Text { text, .. } if text == "hello")
                ));
            }
            _ => panic!("expected user message"),
        },
        _ => panic!("expected Message entry"),
    }
}

#[tokio::test]
async fn test_pg_store_overwrite() {
    let _ = tracing_subscriber::fmt().try_init();
    let (pool, _container) = start_pg().await;
    let store = PgSessionStore::new(pool);
    store.init().await.expect("init failed");

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
                api: ai_provider::Api {
                    provider: "test".to_string(),
                    model: "test".to_string(),
                },
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

    store
        .save_session(tenant, session, &entries_v1)
        .await
        .expect("save v1 failed");
    store
        .save_session(tenant, session, &entries_v2)
        .await
        .expect("save v2 failed");

    let loaded = store
        .load_session(tenant, session)
        .await
        .expect("load failed");
    assert_eq!(loaded.len(), 2);
    match &loaded[1] {
        SessionEntry::Message { message, .. } => match message {
            agent_core::AgentMessage::Assistant(a) => {
                assert!(a.content.iter().any(
                    |c| matches!(c, ai_provider::Content::Text { text, .. } if text == "second")
                ));
            }
            _ => panic!("expected assistant message"),
        },
        _ => panic!("expected Message entry"),
    }
}

#[tokio::test]
async fn test_pg_store_tenant_isolation() {
    let _ = tracing_subscriber::fmt().try_init();
    let (pool, _container) = start_pg().await;
    let store = PgSessionStore::new(pool);
    store.init().await.expect("init failed");

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

    store
        .save_session("tenant_a", "session_1", &entries_a)
        .await
        .expect("save a failed");
    store
        .save_session("tenant_b", "session_1", &entries_b)
        .await
        .expect("save b failed");

    let loaded_a = store
        .load_session("tenant_a", "session_1")
        .await
        .expect("load a failed");
    let loaded_b = store
        .load_session("tenant_b", "session_1")
        .await
        .expect("load b failed");

    assert_eq!(loaded_a.len(), 1);
    assert_eq!(loaded_b.len(), 1);

    match &loaded_a[0] {
        SessionEntry::Message { message, .. } => {
            match message {
                agent_core::AgentMessage::User(u) => {
                    assert!(u.content.iter().any(|c| matches!(c, ai_provider::Content::Text { text, .. } if text == "tenant_a")));
                }
                _ => panic!("expected user message"),
            }
        }
        _ => panic!("expected Message entry"),
    }

    match &loaded_b[0] {
        SessionEntry::Message { message, .. } => {
            match message {
                agent_core::AgentMessage::User(u) => {
                    assert!(u.content.iter().any(|c| matches!(c, ai_provider::Content::Text { text, .. } if text == "tenant_b")));
                }
                _ => panic!("expected user message"),
            }
        }
        _ => panic!("expected Message entry"),
    }
}

#[tokio::test]
async fn test_session_actor_persistence_loop() {
    let _ = tracing_subscriber::fmt().try_init();
    let (pool, _container) = start_pg().await;
    let store = Arc::new(PgSessionStore::new(pool));
    store.init().await.expect("init failed");

    let provider = TestProvider::text("pong");
    let dispatcher = Arc::new(AllowAllDispatcher);

    let tenant = "loop_t";
    let session = "loop_s";

    // 1. Create session, prompt, flush
    {
        let mut session_actor = SessionActor::new(SessionConfig {
            tenant_id: tenant.to_string(),
            session_id: session.to_string(),
            system_prompt: "You are helpful.".to_string(),
            model: "echo".to_string(),
            provider: provider.clone(),
            hook_dispatcher: dispatcher.clone(),
            compaction_actor: make_compaction_actor(provider.clone()),
            tools: vec![],
            store: Some(store.clone()),
            skills: vec![],
        });

        session_actor
            .prompt("ping".to_string())
            .await
            .expect("prompt failed");
        session_actor.flush().await.expect("flush failed");
    }

    // 2. Restore into new session actor
    let mut session2 = SessionActor::new(SessionConfig {
        tenant_id: tenant.to_string(),
        session_id: session.to_string(),
        system_prompt: "You are helpful.".to_string(),
        model: "echo".to_string(),
        provider: provider.clone(),
        hook_dispatcher: dispatcher,
        compaction_actor: make_compaction_actor(provider),
        tools: vec![],
        store: Some(store.clone()),
        skills: vec![],
    });

    let restored = session2.restore().await.expect("restore failed");
    assert!(restored > 0, "expected restored entries > 0");

    let msgs = session2.messages();
    assert!(
        msgs.len() >= 2,
        "expected at least user + assistant messages"
    );

    // Verify the user message survived roundtrip
    assert!(msgs.iter().any(|m| {
        if let agent_core::AgentMessage::User(u) = m {
            u.content
                .iter()
                .any(|c| matches!(c, ai_provider::Content::Text { text, .. } if text == "ping"))
        } else {
            false
        }
    }));
}

#[tokio::test]
async fn test_pg_store_delete() {
    let _ = tracing_subscriber::fmt().try_init();
    let (pool, _container) = start_pg().await;
    let store = PgSessionStore::new(pool);
    store.init().await.expect("init failed");

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

    store
        .save_session(tenant, session, &entries)
        .await
        .expect("save failed");
    let loaded_before = store
        .load_session(tenant, session)
        .await
        .expect("load before failed");
    assert_eq!(loaded_before.len(), 1);

    store
        .delete_session(tenant, session)
        .await
        .expect("delete failed");
    let loaded_after = store
        .load_session(tenant, session)
        .await
        .expect("load after failed");
    assert!(loaded_after.is_empty(), "expected session to be deleted");
}

#[tokio::test]
async fn test_pg_store_list() {
    let _ = tracing_subscriber::fmt().try_init();
    let (pool, _container) = start_pg().await;
    let store = PgSessionStore::new(pool);
    store.init().await.expect("init failed");

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

    store
        .save_session(tenant, "s1", &entries)
        .await
        .expect("save s1 failed");
    store
        .save_session(tenant, "s2", &entries)
        .await
        .expect("save s2 failed");
    store
        .save_session(tenant, "s3", &entries)
        .await
        .expect("save s3 failed");

    let mut sids = store.list_sessions(tenant).await.expect("list failed");
    sids.sort();
    assert_eq!(sids, vec!["s1", "s2", "s3"]);
}

#[tokio::test]
async fn test_pg_store_tenant_list_isolation() {
    let _ = tracing_subscriber::fmt().try_init();
    let (pool, _container) = start_pg().await;
    let store = PgSessionStore::new(pool);
    store.init().await.expect("init failed");

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

    store
        .save_session("tenant_x", "sx1", &entries)
        .await
        .expect("save x failed");
    store
        .save_session("tenant_x", "sx2", &entries)
        .await
        .expect("save x2 failed");
    store
        .save_session("tenant_y", "sy1", &entries)
        .await
        .expect("save y failed");

    let list_x = store
        .list_sessions("tenant_x")
        .await
        .expect("list x failed");
    let mut list_x_sorted = list_x.clone();
    list_x_sorted.sort();
    assert_eq!(list_x_sorted, vec!["sx1", "sx2"]);

    let list_y = store
        .list_sessions("tenant_y")
        .await
        .expect("list y failed");
    assert_eq!(list_y, vec!["sy1"]);
}

#[tokio::test]
async fn test_pg_append_entries() {
    let _ = tracing_subscriber::fmt().try_init();
    let (pool, _container) = start_pg().await;
    let store = PgSessionStore::new(pool);
    store.init().await.expect("init failed");

    let tenant = "append_t";
    let session = "append_s";

    // 1. Save initial entries
    let e1 = SessionEntry::Message {
        id: uuid::Uuid::new_v4(),
        message: agent_core::AgentMessage::User(ai_provider::UserMessage {
            content: vec![ai_provider::Content::Text {
                text: "first".to_string(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        }),
    };
    store
        .save_session(tenant, session, &[e1])
        .await
        .expect("save failed");

    // 2. Append new entries
    let e2 = SessionEntry::Message {
        id: uuid::Uuid::new_v4(),
        message: agent_core::AgentMessage::Assistant(ai_provider::AssistantMessage {
            content: vec![ai_provider::Content::Text {
                text: "second".to_string(),
                text_signature: None,
            }],
            provider: "test".into(),
            model: "test".into(),
            api: ai_provider::Api {
                provider: "test".into(),
                model: "test".into(),
            },
            usage: ai_provider::Usage {
                input_tokens: 1,
                output_tokens: 1,
                total_tokens: 2,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
            stop_reason: ai_provider::StopReason::Stop,
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::now(),
        }),
    };
    store
        .append_entries(tenant, session, &[e2])
        .await
        .expect("append failed");

    // 3. Load and verify both entries exist
    let loaded = store
        .load_session(tenant, session)
        .await
        .expect("load failed");
    assert_eq!(loaded.len(), 2, "expected 2 entries after append");
}
