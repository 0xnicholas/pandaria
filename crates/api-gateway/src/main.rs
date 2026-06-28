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
    let runtime_config = Arc::new(agent_core::HarnessConfig::from_env(provider.clone()));
    info!(
        default_model = %runtime_config.default_model,
        available_models = ?runtime_config.available_models,
        compaction_enabled = runtime_config.compaction_config.enabled,
        "harness config loaded from env"
    );

    // --- 4. Tenant Manager ---
    let tenant_manager: Arc<dyn tenant::TenantManager> = Arc::new(
        tenant::manager::TenantManagerImpl::new(registry.clone(), runtime_config.clone(), None),
    );

    // --- 5. Server Config + Aspectus ---
    let config = ServerConfig::from_env();
    let aspectus_config = api_gateway::config::AspectusConfig::from_env()?;

    let state = Arc::new(AppState::new(
        tenant_manager,
        config.clone(),
        registry,
        &aspectus_config,
    )?);

    // --- 6. Tavern Workflow Engine ---
    // SSRF policy: reuse the one loaded by HarnessConfig (from
    // PANDARIA_SSRF_ALLOWLIST). Strict by default; set the env var to allow
    // specific CIDR ranges and domains (e.g. for pandaria ↔ DayPaw integration).
    let ssrf_policy = runtime_config.ssrf_policy.clone();
    let tavern_runtime = Arc::new(tavern_comp::AgentRuntime::new_with_ssrf_policy(
        ssrf_policy.clone(),
    ));
    let hero = tavern_comp::TavernHero::new(tavern_runtime);
    let agent_config_dir = std::path::Path::new("./configs/agents");
    if agent_config_dir.exists()
        && let Err(e) = hero.load_from_dir(agent_config_dir).await
    {
        tracing::error!("failed to load agent configs: {}", e);
    }
    let hero = Arc::new(hero);
    let mut workflow_registry = tavern_comp::WorkflowRegistry::new();
    let workflow_config_dir = std::path::Path::new("./configs/workflows");
    if workflow_config_dir.exists()
        && let Err(e) = workflow_registry.load_from_dir(workflow_config_dir)
    {
        tracing::error!("failed to load workflow configs: {}", e);
    }
    let workflow_registry = Arc::new(tokio::sync::RwLock::new(workflow_registry));
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
        registry: workflow_registry.clone(),
        event_store,
        tool_registry,
        squads: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
    });

    // --- 7. Print startup info ---
    println!("========================================");
    println!("  pandaria-server ready");
    println!("  bind: {}", config.bind_addr);
    println!("  auth:  Aspectus ({})", aspectus_config.base_url);
    println!(
        "  ssrf:  allowlist_enabled={} entries={}",
        ssrf_policy.allowlist_enabled(),
        ssrf_policy.allowlist_size(),
    );
    println!("========================================");

    serve(state, Some(tavern_state)).await
}
