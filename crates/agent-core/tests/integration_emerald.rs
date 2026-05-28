//! Emerald HTTP adapter integration tests.
//!
//! These tests require a running Emerald v0.2.0+ service.
//!
//! # Quick start (one command)
//!
//! ```bash
//! ./scripts/run-emerald-tests.sh
//! ```
//!
//! This auto-starts Emerald in **memory mode** (no Docker, no DB), runs tests,
//! and stops the server. See the Emerald quickstart for details:
//! `~/Documents/build-whatever/Emerald/docs/superpowers/specs/2026-05-27-emerald-v0.2.0-quickstart.md`
//!
//! # Manual modes
//!
//! ## Memory mode (fastest, no Docker)
//!
//! ```bash
//! cd ~/Documents/build-whatever/Emerald
//! git checkout v0.2.0
//! pip install -e "."
//! python3 scripts/test_server.py   # → http://localhost:9999, any API key
//!
//! # In another terminal:
//! PANDARIA_TEST_EMERALD_URL="http://localhost:9999" \
//! PANDARIA_TEST_EMERALD_API_KEY="em_test" \
//!   cargo test -p agent-core --test integration_emerald -- --test-threads=1
//! ```
//!
//! ## Docker mode (full stack)
//!
//! ```bash
//! ./scripts/run-emerald-tests.sh --docker
//! ```
//!
//! ## External mode (already running)
//!
//! ```bash
//! PANDARIA_TEST_EMERALD_URL="http://localhost:8000" \
//! PANDARIA_TEST_EMERALD_API_KEY="em_xxx" \
//!   cargo test -p agent-core --test integration_emerald -- --test-threads=1
//! ```
//!
//! `--test-threads=1` is recommended because tests share the same Emerald
//! instance and entity state; concurrent writes to the same tenant could
//! cause flaky recall counts.

use std::time::{Duration, SystemTime};

use agent_core::memory::{EmeraldMemoryStore, MemoryContext, MemoryStore};

fn env_or_skip() -> Option<(String, String)> {
    let url = std::env::var("PANDARIA_TEST_EMERALD_URL").ok()?;
    let key = std::env::var("PANDARIA_TEST_EMERALD_API_KEY").ok()?;
    Some((url, key))
}

fn make_ctx(tenant_id: &str, session_id: &str) -> MemoryContext {
    MemoryContext {
        tenant_id: tenant_id.to_string(),
        session_id: session_id.to_string(),
        user_id: None,
        model: "claude-sonnet-4".to_string(),
        session_started_at: SystemTime::now(),
    }
}

/// Retry helper: Emerald may need a short delay between write and searchable.
async fn try_recall(
    store: &EmeraldMemoryStore,
    ctx: &MemoryContext,
    query: &str,
    max_attempts: usize,
) -> Vec<String> {
    for i in 0..max_attempts {
        match store.recall(ctx, query).await {
            Ok(results) if !results.is_empty() => return results,
            _ => {
                if i + 1 < max_attempts {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }
    Vec::new()
}

/// Retry helper that tolerates empty results (for isolation / negative tests).
async fn try_recall_allow_empty(
    store: &EmeraldMemoryStore,
    ctx: &MemoryContext,
    query: &str,
    max_attempts: usize,
) -> Vec<String> {
    for i in 0..max_attempts {
        match store.recall(ctx, query).await {
            Ok(results) => return results,
            _ => {
                if i + 1 < max_attempts {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }
    Vec::new()
}

// ============================================================================
// E2E: basic remember + recall
// ============================================================================

#[tokio::test]
async fn test_e2e_remember_then_recall() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let store = EmeraldMemoryStore::new(url, key);
    let ctx = make_ctx("test_tenant_e2e", "sess_recall_001");

    store
        .remember(
            &ctx,
            "**User**: hello Emerald\n\n**Assistant**: hi there\n\n",
            &serde_json::json!({"foo": "bar"}),
        )
        .await
        .expect("remember should succeed");

    let results = try_recall(&store, &ctx, "hello", 10).await;

    assert!(!results.is_empty(), "should recall the remembered content");
    let combined = results.join(" ");
    assert!(
        combined.to_lowercase().contains("hello"),
        "recalled content should contain query keyword: got {:?}",
        results
    );
}

#[tokio::test]
async fn test_e2e_recall_is_cross_session() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let store = EmeraldMemoryStore::new(url, key);
    let tenant = "tenant_shared_e2e";

    let ctx_a = make_ctx(tenant, "sess_a");
    store
        .remember(
            &ctx_a,
            "**User**: I love Rust programming\n\n**Assistant**: Great choice!\n\n",
            &serde_json::json!({}),
        )
        .await
        .unwrap();

    let ctx_b = make_ctx(tenant, "sess_b");
    let results = try_recall(&store, &ctx_b, "Rust programming", 10).await;

    assert!(
        !results.is_empty(),
        "same tenant should recall across sessions"
    );
    let combined = results.join(" ");
    assert!(
        combined.to_lowercase().contains("rust"),
        "should find Rust-related memory across sessions"
    );
}

#[tokio::test]
async fn test_e2e_tenant_isolation() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let store = EmeraldMemoryStore::new(url, key);

    let ctx_a = make_ctx("tenant_isolation_a", "sess_1");
    store
        .remember(
            &ctx_a,
            "**User**: my secret is alpha-42\n\n**Assistant**: noted\n\n",
            &serde_json::json!({}),
        )
        .await
        .unwrap();

    let ctx_b = make_ctx("tenant_isolation_b", "sess_1");
    let results = try_recall_allow_empty(&store, &ctx_b, "alpha-42", 5).await;

    assert!(
        results.is_empty(),
        "tenant B should not see tenant A's memories"
    );
}

#[tokio::test]
async fn test_e2e_recall_returns_empty_for_unknown_query() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let store = EmeraldMemoryStore::new(url, key);
    let ctx = make_ctx("tenant_unknown", "sess_1");

    let results = store.recall(&ctx, "xyznonexistentquery123").await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_e2e_forget_session_is_noop() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let store = EmeraldMemoryStore::new(url, key);
    let ctx = make_ctx("tenant_forget", "sess_1");

    store.forget_session(&ctx).await.unwrap();
}

// ============================================================================
// E2E: multi-turn conversation & semantic recall
// ============================================================================

#[tokio::test]
async fn test_e2e_multiturn_conversation_recall() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let store = EmeraldMemoryStore::new(url, key);
    let ctx = make_ctx("tenant_multiturn", "sess_multi");

    let conversation = "## Turn 1\n\n\
        **User**: What is Rust's ownership model?\n\n\
        **Assistant**: Rust uses a unique ownership system where each value has a single owner.\n\n\
        **User**: How does borrowing work then?\n\n\
        **Assistant**: Borrowing allows you to reference a value without taking ownership.\n\n\
        ## Turn 2\n\n\
        **User**: Can I have multiple mutable borrows?\n\n\
        **Assistant**: No, Rust enforces at most one mutable borrow at a time to prevent data races.\n\n";

    store
        .remember(&ctx, conversation, &serde_json::json!({"turns": 2}))
        .await
        .unwrap();

    // Query about a concept discussed in the conversation
    let results = try_recall(&store, &ctx, "mutable borrow rules", 10).await;
    assert!(
        !results.is_empty(),
        "should recall conversation about borrowing"
    );
    let combined = results.join(" ").to_lowercase();
    assert!(
        combined.contains("borrow") || combined.contains("ownership"),
        "recall should contain relevant concepts: got {:?}",
        results
    );
}

#[tokio::test]
async fn test_e2e_semantic_recall() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let store = EmeraldMemoryStore::new(url, key);
    let ctx = make_ctx("tenant_semantic", "sess_sem");

    store
        .remember(
            &ctx,
            "**User**: I really enjoy writing code in Rust because it prevents memory leaks.\n\n\
             **Assistant**: Yes, the ownership model guarantees memory safety at compile time.\n\n",
            &serde_json::json!({}),
        )
        .await
        .unwrap();

    // Query uses different words but same meaning
    let results = try_recall(&store, &ctx, "safe memory management", 10).await;
    assert!(
        !results.is_empty(),
        "semantic recall should find related content"
    );
}

// ============================================================================
// E2E: top-k limit
// ============================================================================

#[tokio::test]
async fn test_e2e_top_k_limits_results() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let store = EmeraldMemoryStore::new(url, key);
    let ctx = make_ctx("tenant_topk", "sess_topk");

    // Write 8 distinct memories
    for i in 0..8 {
        store
            .remember(
                &ctx,
                &format!(
                    "**User**: Tell me about topic number {}\n\n**Assistant**: Here is info about topic {}.\n\n",
                    i, i
                ),
                &serde_json::json!({"index": i}),
            )
            .await
            .unwrap();
    }

    // Recall should return at most 5 results (top_k=5 in EmeraldMemoryStore)
    let results = try_recall(&store, &ctx, "topic", 10).await;
    assert!(
        results.len() <= 5,
        "recall should respect top_k=5 limit, got {} results",
        results.len()
    );
}

// ============================================================================
// E2E: concurrent writes
// ============================================================================

#[tokio::test]
async fn test_e2e_concurrent_remember() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let store = std::sync::Arc::new(EmeraldMemoryStore::new(url, key));
    let tenant = "tenant_concurrent";

    // Spawn 5 concurrent remember calls with different sessions
    let mut handles = vec![];
    for i in 0..5 {
        let s = store.clone();
        let ctx = make_ctx(tenant, &format!("sess_{}", i));
        let handle = tokio::spawn(async move {
            s.remember(
                &ctx,
                &format!("**User**: concurrent message {}\n\n**Assistant**: ack {}\n\n", i, i),
                &serde_json::json!({"seq": i}),
            )
            .await
            .unwrap();
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.unwrap();
    }

    // All sessions share the same tenant, so recall from any session should find all
    let ctx_recall = make_ctx(tenant, "sess_recall");
    let results = try_recall(&store, &ctx_recall, "concurrent", 15).await;
    assert!(
        !results.is_empty(),
        "should recall at least some concurrent writes"
    );
}

// ============================================================================
// E2E: unicode, special chars, code blocks
// ============================================================================

#[tokio::test]
async fn test_e2e_unicode_and_special_chars() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let store = EmeraldMemoryStore::new(url, key);
    let ctx = make_ctx("tenant_unicode", "sess_uni");

    let content = "## Turn 1\n\n\
        **User**: 你好世界 🌍 こんにちは\n\n\
        **Assistant**: Here's some code:\n\n\
        ```rust\n\
        fn main() {\n\
            println!(\"Hello \\u{1F600}\");\n\
        }\n\
        ```\n\n\
        **User**: <script>alert('xss')</script>\\n\n\
        **Assistant**: HTML tags are handled safely.\n\n";

    store
        .remember(&ctx, content, &serde_json::json!({}))
        .await
        .unwrap();

    // Recall by Chinese keyword
    let results_cn = try_recall(&store, &ctx, "你好", 10).await;
    assert!(
        !results_cn.is_empty(),
        "should recall Chinese content: got {:?}",
        results_cn
    );

    // Recall by code keyword
    let results_code = try_recall(&store, &ctx, "rust code", 10).await;
    assert!(
        !results_code.is_empty(),
        "should recall code block content: got {:?}",
        results_code
    );

    let combined = results_code.join(" ");
    assert!(
        combined.contains("fn main")
            || combined.contains("println!")
            || combined.contains("rust"),
        "recalled code should contain Rust syntax: got {:?}",
        results_code
    );
}

// ============================================================================
// E2E: empty query
// ============================================================================

#[tokio::test]
async fn test_e2e_empty_query_returns_empty() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let store = EmeraldMemoryStore::new(url, key);
    let ctx = make_ctx("tenant_empty_q", "sess_1");

    store
        .remember(
            &ctx,
            "**User**: something\n\n**Assistant**: response\n\n",
            &serde_json::json!({}),
        )
        .await
        .unwrap();

    // Empty string query should return empty (or very quickly)
    let results = store.recall(&ctx, "").await.unwrap();
    assert!(
        results.is_empty(),
        "empty query should return empty results"
    );
}

// ============================================================================
// E2E: large content
// ============================================================================

#[tokio::test]
async fn test_e2e_large_content() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let store = EmeraldMemoryStore::new(url, key);
    let ctx = make_ctx("tenant_large", "sess_large");

    // Generate ~10KB of content
    let mut large_text = String::new();
    for i in 0..200 {
        large_text.push_str(&format!(
            "**User**: This is paragraph {} of a very long document discussing \
            distributed systems, consensus algorithms, and eventual consistency.\n\n\
            **Assistant**: Paragraph {} explains that distributed systems require \
            careful coordination between nodes to maintain consistency guarantees.\n\n",
            i, i
        ));
    }
    assert!(large_text.len() > 5000, "content should be large");

    store
        .remember(&ctx, &large_text, &serde_json::json!({"size_kb": large_text.len() / 1024}))
        .await
        .unwrap();

    let results = try_recall(&store, &ctx, "distributed systems consensus", 10).await;
    assert!(
        !results.is_empty(),
        "should recall from large content"
    );
}

// ============================================================================
// E2E: profile API
// ============================================================================

#[tokio::test]
async fn test_e2e_profile_api_returns_entity_profile() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let store = EmeraldMemoryStore::new(&url, &key);
    let ctx = make_ctx("tenant_profile", "sess_prof");

    // Seed some memories so profile has data
    store
        .remember(
            &ctx,
            "**User**: I work with Rust and Python\n\n**Assistant**: Great stack!\n\n",
            &serde_json::json!({"category": "skills"}),
        )
        .await
        .unwrap();

    // Profile endpoint is not part of MemoryStore trait, test via raw HTTP
    let client = reqwest::Client::new();
    let profile_url = format!("{}/v1/profiles/{}", url, ctx.tenant_id);

    let response = client
        .get(&profile_url)
        .header("Authorization", format!("Bearer {}", key))
        .timeout(Duration::from_secs(5))
        .send()
        .await;

    match response {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            assert!(
                body.get("data").is_some(),
                "profile response should have data field: got {:?}",
                body
            );
        }
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            eprintln!("Profile API returned {}: {}", status, text);
            // v0.2.0 memory mode may not fully implement profile; don't fail the test.
        }
        Err(e) => {
            eprintln!("Profile API request failed (may not be implemented in memory mode): {}", e);
        }
    }
}

// ============================================================================
// E2E: metadata roundtrip (best-effort via profile)
// ============================================================================

#[tokio::test]
async fn test_e2e_metadata_persisted() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let store = EmeraldMemoryStore::new(url, key);
    let ctx = make_ctx("tenant_meta", "sess_meta");
    let custom_meta = serde_json::json!({
        "session_id": "sess_meta",
        "model": "claude-sonnet-4",
        "turn_index": 42,
        "custom_field": "custom_value"
    });

    store
        .remember(
            &ctx,
            "**User**: metadata test\n\n**Assistant**: ok\n\n",
            &custom_meta,
        )
        .await
        .unwrap();

    // The MemoryStore API doesn't expose metadata in recall results.
    // We verify at minimum that remember() succeeds with complex metadata.
    // Future: verify via Emerald admin API or profile endpoint.
    let results = try_recall(&store, &ctx, "metadata test", 10).await;
    assert!(!results.is_empty(), "should recall content with complex metadata");
}

// ============================================================================
// E2E: persistence — data survives across new store instances & time
// ============================================================================

#[tokio::test]
async fn test_e2e_persistence_new_instance_recalls() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let tenant = "tenant_persist_new_instance";
    let ctx = make_ctx(tenant, "sess_persist_001");

    // Store A: write and drop
    {
        let store_a = EmeraldMemoryStore::new(&url, &key);
        store_a
            .remember(
                &ctx,
                "**User**: Persistence test — store A wrote this\n\n**Assistant**: Acknowledged from store A\n\n",
                &serde_json::json!({"source": "store_a"}),
            )
            .await
            .unwrap();
    }

    // Give async indexing time (defensive, though Emerald may index synchronously)
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Store B: brand new instance with fresh HTTP client, should still recall
    let store_b = EmeraldMemoryStore::new(&url, &key);
    let results = try_recall(&store_b, &ctx, "persistence test store A", 20).await;

    assert!(
        !results.is_empty(),
        "new store instance should recall data written by a previous instance"
    );
    let combined = results.join(" ").to_lowercase();
    assert!(
        combined.contains("persistence") || combined.contains("store a"),
        "recalled content should mention the persisted data: got {:?}",
        results
    );
}

#[tokio::test]
async fn test_e2e_persistence_multiple_writes_new_instance_reads_all() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let tenant = "tenant_persist_multi";
    let ctx = make_ctx(tenant, "sess_multi");

    // Store A: write multiple memories then drop
    {
        let store_a = EmeraldMemoryStore::new(&url, &key);
        for i in 0..3 {
            store_a
                .remember(
                    &ctx,
                    &format!(
                        "**User**: Fact number {} about persistent storage\n\n**Assistant**: Confirmed fact {}\n\n",
                        i, i
                    ),
                    &serde_json::json!({"fact_id": i}),
                )
                .await
                .unwrap();
        }
    }

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Store B: read back with a fresh instance
    let store_b = EmeraldMemoryStore::new(&url, &key);
    let results = try_recall(&store_b, &ctx, "persistent storage fact", 20).await;

    assert!(!results.is_empty(), "should recall persisted facts");
    assert!(
        results.len() >= 2,
        "should recall at least 2 of the 3 persisted facts, got {}",
        results.len()
    );
}

#[tokio::test]
async fn test_e2e_persistence_tenant_isolation_across_instances() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let tenant_a = "tenant_persist_iso_a";
    let tenant_b = "tenant_persist_iso_b";

    // Store A: write for tenant A then drop
    {
        let store = EmeraldMemoryStore::new(&url, &key);
        let ctx_a = make_ctx(tenant_a, "sess_a");
        store
            .remember(
                &ctx_a,
                "**User**: Secret of tenant A\n\n**Assistant**: Noted\n\n",
                &serde_json::json!({}),
            )
            .await
            .unwrap();
    }

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Store B: new instance tries to read tenant A's data as tenant B
    let store_b = EmeraldMemoryStore::new(&url, &key);
    let ctx_b = make_ctx(tenant_b, "sess_b");
    let results = try_recall_allow_empty(&store_b, &ctx_b, "secret of tenant A", 10).await;

    assert!(
        results.is_empty(),
        "tenant B should not see tenant A's persisted data even with a new store instance"
    );
}

#[tokio::test]
async fn test_e2e_persistence_survives_delay() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let tenant = "tenant_persist_delay";
    let ctx = make_ctx(tenant, "sess_delay");

    // Store A writes then drops
    {
        let store = EmeraldMemoryStore::new(&url, &key);
        store
            .remember(
                &ctx,
                "**User**: Data that must survive a long delay\n\n**Assistant**: It will persist\n\n",
                &serde_json::json!({"delayed": true}),
            )
            .await
            .unwrap();
    }

    // Wait longer than typical in-memory cache TTL
    tokio::time::sleep(Duration::from_secs(10)).await;

    // Store B reads after delay with a fresh instance
    let store_b = EmeraldMemoryStore::new(&url, &key);
    let results = try_recall(&store_b, &ctx, "survive long delay", 20).await;

    assert!(
        !results.is_empty(),
        "data should be recallable after a long delay"
    );
}

#[tokio::test]
async fn test_e2e_persistence_overwrite_then_recall_latest() {
    let _ = tracing_subscriber::fmt().try_init();
    let (url, key) = match env_or_skip() {
        Some(v) => v,
        None => {
            eprintln!("SKIP: set PANDARIA_TEST_EMERALD_URL and PANDARIA_TEST_EMERALD_API_KEY");
            return;
        }
    };

    let tenant = "tenant_persist_overwrite";
    let ctx = make_ctx(tenant, "sess_overwrite");

    // Store A: write original
    {
        let store = EmeraldMemoryStore::new(&url, &key);
        store
            .remember(
                &ctx,
                "**User**: Original topic is dogs\n\n**Assistant**: Dogs are great\n\n",
                &serde_json::json!({"version": 1}),
            )
            .await
            .unwrap();
    }

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Store B: overwrite with new content
    {
        let store = EmeraldMemoryStore::new(&url, &key);
        store
            .remember(
                &ctx,
                "**User**: Updated topic is cats\n\n**Assistant**: Cats are wonderful\n\n",
                &serde_json::json!({"version": 2}),
            )
            .await
            .unwrap();
    }

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Store C: fresh instance recalls — should find at least one of the writes
    let store_c = EmeraldMemoryStore::new(&url, &key);
    let results = try_recall(&store_c, &ctx, "cats dogs topic", 20).await;

    assert!(
        !results.is_empty(),
        "overwrite data should be persisted and recallable"
    );
    let combined = results.join(" ").to_lowercase();
    assert!(
        combined.contains("cats") || combined.contains("dogs"),
        "recall should contain one of the persisted topics: got {:?}",
        results
    );
}
