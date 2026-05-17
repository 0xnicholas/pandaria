use agent_core::{SessionEntry, SessionStore};
use storage::session::redis::RedisSessionStore;
use redis::aio::MultiplexedConnection;
use testcontainers_modules::redis::Redis;
use testcontainers_modules::testcontainers::runners::AsyncRunner;

/// Start a Redis container and return a multiplexed connection.
async fn start_redis() -> (MultiplexedConnection, testcontainers_modules::testcontainers::ContainerAsync<Redis>) {
    let container = Redis::default().start().await.expect("failed to start redis container");
    let host = container.get_host().await.expect("failed to get container host");
    let port = container.get_host_port_ipv4(6379).await.expect("failed to get container port");
    let url = format!("redis://{}:{}", host, port);

    let client = redis::Client::open(url).expect("invalid redis url");
    let conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("failed to connect to testcontainers redis");

    (conn, container)
}


#[tokio::test]
async fn test_redis_store_roundtrip() {
    let _ = tracing_subscriber::fmt().try_init();
    let (conn, _container) = start_redis().await;
    let store = RedisSessionStore::new(conn);

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
async fn test_redis_store_overwrite() {
    let _ = tracing_subscriber::fmt().try_init();
    let (conn, _container) = start_redis().await;
    let store = RedisSessionStore::new(conn);

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
async fn test_redis_store_tenant_isolation() {
    let _ = tracing_subscriber::fmt().try_init();
    let (conn, _container) = start_redis().await;
    let store = RedisSessionStore::new(conn);

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
async fn test_redis_store_delete() {
    let _ = tracing_subscriber::fmt().try_init();
    let (conn, _container) = start_redis().await;
    let store = RedisSessionStore::new(conn);

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
async fn test_redis_store_list() {
    let _ = tracing_subscriber::fmt().try_init();
    let (conn, _container) = start_redis().await;
    let store = RedisSessionStore::new(conn);

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
async fn test_redis_store_tenant_list_isolation() {
    let _ = tracing_subscriber::fmt().try_init();
    let (conn, _container) = start_redis().await;
    let store = RedisSessionStore::new(conn);

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
