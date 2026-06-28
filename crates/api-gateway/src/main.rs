use std::sync::Arc;

use api_gateway::config::ServerConfig;
use api_gateway::server::{AppState, serve};
use tracing::info;

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

    // --- 3. Runtime Config (from environment variables) ---
    let mut runtime_config = agent_core::HarnessConfig::from_env(provider.clone());

    // Wire CPU time callback: each turn end records wall-clock duration
    let cpu_registry = registry.clone();
    runtime_config.hook_config.cpu_time_callback = Some(std::sync::Arc::new(
        move |tenant_id: &str, ms: u64| {
            cpu_registry.record_cpu_time_ms(tenant_id, ms);
        },
    ));

    let runtime_config = Arc::new(runtime_config);
    info!(
        default_model = %runtime_config.default_model,
        available_models = ?runtime_config.available_models,
        compaction_enabled = runtime_config.compaction_config.enabled,
        "harness config loaded from env"
    );

    // --- 4. Tenant Manager ---
    let metrics_registry = Arc::new(observability::MetricsRegistry::new());
    let tenant_manager: Arc<dyn tenant::TenantManager> = Arc::new(
        tenant::manager::TenantManagerImpl::new(
            registry.clone(),
            runtime_config.clone(),
            Some(metrics_registry.clone()),
        ),
    );

    // --- 5. Server Config + Aspectus ---
    let config = ServerConfig::from_env();
    let aspectus_config = api_gateway::config::AspectusConfig::from_env()?;

    let mut state = AppState::new(
        tenant_manager,
        config.clone(),
        registry,
        &aspectus_config,
    )?;
    state.metrics_registry = Some(metrics_registry);
    let state = Arc::new(state);

    // --- 6. Tavern Agent Team ---
    let hero = tavern_comp::TavernHero::new();
    let agent_config_dir = std::path::Path::new("./configs/agents");
    if agent_config_dir.exists()
        && let Err(e) = hero.load_from_dir(agent_config_dir).await
    {
        tracing::error!("failed to load agent configs: {}", e);
    }
    let hero = Arc::new(hero);
    let event_store: Arc<dyn tavern_comp::EventStore> =
        Arc::new(tavern_comp::MemoryEventStore::new());
    let tool_registry = tavern_core::ToolRegistry::new();
    tool_registry.register(
        "web_search".into(),
        std::sync::Arc::new(api_gateway::tavern_tools::web_search::WebSearchHandler::new()),
    );
    let tool_registry = Arc::new(tool_registry);
    let tavern_state = Arc::new(api_gateway::tavern::TavernState {
        hero: hero.clone(),
        event_store,
        tool_registry,
        squads: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
    });

    // --- 7. Print startup info ---
    println!("========================================");
    println!("  pandaria-server ready");
    println!("  bind: {}", config.bind_addr);
    println!("  auth:  Aspectus ({})", aspectus_config.base_url);
    println!("  ssrf:  allowlist_enabled={} entries={}",
        runtime_config.ssrf_policy.allowlist_enabled(),
        runtime_config.ssrf_policy.allowlist_size(),
    );
    println!("========================================");

    serve(state, Some(tavern_state)).await
}
