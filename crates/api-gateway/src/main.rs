use std::sync::Arc;

use api_gateway::config::ServerConfig;
use api_gateway::server::{AppState, serve};
use secrecy::ExposeSecret;
use tracing::info;

/// Generate an HMAC-signed token for the given tenant.
fn generate_token(secret: &str, tenant_id: &str) -> String {
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

    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(&payload_json);
    let signature = mac.finalize().into_bytes();
    let sig_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        signature,
    );

    format!("{}.{}", payload_b64, sig_b64)
}

/// Register a dev tenant when `PANDARIA_DEV_TENANT` is set.
/// The tenant ID and quota are read from environment variables.
fn register_dev_tenant(registry: &tenant::TenantRegistry) -> Option<String> {
    let tenant_id = std::env::var("PANDARIA_DEV_TENANT").ok()?;

    let max_concurrent = std::env::var("PANDARIA_DEV_TENANT_MAX_SESSIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);

    let max_tokens = std::env::var("PANDARIA_DEV_TENANT_MAX_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000_000);

    let max_tool_calls = std::env::var("PANDARIA_DEV_TENANT_MAX_TOOL_CALLS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);

    let cpu_budget = std::env::var("PANDARIA_DEV_TENANT_CPU_BUDGET_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3_600_000);

    let quota = tenant::TenantQuota {
        max_concurrent_sessions: max_concurrent,
        max_tokens_per_day: max_tokens,
        max_tool_calls_per_minute: max_tool_calls,
        cpu_time_budget_ms_per_day: cpu_budget,
    };

    let t = tenant::Tenant::new(&tenant_id, quota);
    match registry.register(t) {
        Ok(()) => {
            info!(%tenant_id, "registered dev tenant");
            Some(tenant_id)
        }
        Err(e) => {
            tracing::warn!(%tenant_id, error = %e, "failed to register dev tenant");
            None
        }
    }
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
    let provider: Arc<dyn ai_provider::LlmProvider> = Arc::new(ai_provider::RouterProvider::new());

    // --- 2. Tenant Registry ---
    let registry = Arc::new(tenant::TenantRegistry::new());
    let dev_tenant_id = register_dev_tenant(&registry);

    // --- 3. Runtime Config (from environment variables) ---
    let runtime_config = Arc::new(agent_core::HarnessConfig::from_env(provider.clone()));
    info!(
        default_model = %runtime_config.default_model,
        available_models = ?runtime_config.available_models,
        compaction_enabled = runtime_config.compaction_config.enabled,
        "harness config loaded from env"
    );

    // --- 4. Tenant Manager ---
    let tenant_manager: Arc<dyn tenant::TenantManager> = Arc::new(
        tenant::manager::TenantManagerImpl::new(registry, runtime_config),
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

    // --- 6. Tavern Workflow Engine ---
    let tavern_runtime = Arc::new(tavern_comp::AgentRuntime::new());
    let hero = tavern_comp::TavernHero::new(tavern_runtime);
    let agent_config_dir = std::path::Path::new("./configs/agents");
    if agent_config_dir.exists() {
        if let Err(e) = hero.load_from_dir(agent_config_dir).await {
            tracing::error!("failed to load agent configs: {}", e);
        }
    }
    let hero = Arc::new(hero);
    let mut workflow_registry = tavern_comp::WorkflowRegistry::new();
    let workflow_config_dir = std::path::Path::new("./configs/workflows");
    if workflow_config_dir.exists() {
        if let Err(e) = workflow_registry.load_from_dir(workflow_config_dir) {
            tracing::error!("failed to load workflow configs: {}", e);
        }
    }
    let workflow_registry = Arc::new(tokio::sync::RwLock::new(workflow_registry));
    let event_store: Arc<dyn tavern_comp::EventStore> = Arc::new(tavern_comp::MemoryEventStore::new());
    let tavern_state = Arc::new(api_gateway::tavern::TavernState {
        hero: hero.clone(),
        registry: workflow_registry.clone(),
        event_store,
        tool_registry: Arc::new(tavern_core::ToolRegistry::new()),
    });

    // --- 7. Print startup info ---
    println!("========================================");
    println!("  pandaria-server ready");
    println!("  bind: {}", config.bind_addr);
    if let Some(ref tenant_id) = dev_tenant_id {
        let secret = config.auth_secret.expose_secret();
        let token = generate_token(secret, tenant_id);
        println!("  dev tenant: {tenant_id}");
        println!("  dev token:  {token}");
    }
    println!("========================================");

    serve(state, Some(tavern_state)).await
}
