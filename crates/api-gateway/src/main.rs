use std::sync::Arc;

use api_gateway::config::ServerConfig;
use api_gateway::server::{serve, AppState};
use secrecy::ExposeSecret;
use tracing::info;

fn generate_test_token(secret: &str, tenant_id: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_secs();

    let payload = serde_json::json!({
        "tenant_id": tenant_id,
        "iat": now,
        "exp": now + 86400, // 24h expiration
    });
    let payload_json = serde_json::to_vec(&payload).expect("json encode");
    let payload_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        &payload_json,
    );

    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(&payload_json);
    let signature = mac.finalize().into_bytes();
    let sig_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        &signature,
    );

    format!("{}.{}", payload_b64, sig_b64)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("pandaria-server starting...");

    // --- 0. Ensure agent space directories exist ---
    let agent_space = agent_core::space::AgentSpace::from_env_or_default();
    if let Err(e) = agent_space.ensure_dirs() {
        eprintln!("warning: failed to create agent space directories: {e}");
    }
    info!(root = %agent_space.root().display(), "agent space ready");

    // --- 1. LLM Provider (RouterProvider auto-routes by model name) ---
    let provider: Arc<dyn ai_provider::LlmProvider> =
        Arc::new(ai_provider::RouterProvider::new());

    // --- 2. Tenant Registry ---
    let registry = Arc::new(tenant::TenantRegistry::new());
    let test_tenant = tenant::Tenant::new(
        "test-tenant",
        tenant::TenantQuota {
            max_concurrent_sessions: 10,
            max_tokens_per_day: 1_000_000,
            max_tool_calls_per_minute: 60,
            cpu_time_budget_ms_per_day: 3_600_000,
        },
    );
    registry.register(test_tenant)?;
    info!("registered test tenant: test-tenant");

    // --- 3. Tenant Manager ---
    let tenant_manager: Arc<dyn tenant::TenantManager> = Arc::new(
        tenant::manager::TenantManagerImpl::new(
            registry,
            provider,
            None,                        // no persistent session store (in-memory only)
            "deepseek/deepseek-v4-pro",  // default model (RouterProvider needs provider/model format)
            "You are a helpful assistant.", // default system prompt
            128_000,                     // default context window
        ),
    );

    // --- 5. Server Config ---
    let config = ServerConfig::from_env();

    if config.is_default_secret() {
        panic!(
            "PANDARIA_AUTH_SECRET is not set. \
             Set it in .env or environment before starting the server."
        );
    }

    let state = Arc::new(AppState::new(tenant_manager, config.clone()));

    // --- 6. Print startup info ---
    println!("========================================");
    println!("  pandaria-server ready");
    println!("  bind: {}", config.bind_addr);
    if std::env::var("PANDARIA_DEV_MODE").is_ok() {
        let secret = config.auth_secret.expose_secret();
        let token = generate_test_token(secret, "test-tenant");
        println!("  tenant: test-tenant");
        println!("  token:  {}", token);
    }
    println!("========================================");

    serve(state).await
}
