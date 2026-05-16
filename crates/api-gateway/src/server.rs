use axum::{
    middleware,
    routing::{delete, get, post},
    Router,
};
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::config::ServerConfig;
use crate::middleware::{auth, rate_limit};
use crate::routes::{events, health, messages, sessions};

/// 应用状态。
pub struct AppState {
    pub tenant_manager: Arc<dyn tenant::TenantManager>,
    pub config: ServerConfig,
    pub rate_limiter: rate_limit::RateLimiter,
}

impl AppState {
    pub fn new(
        tenant_manager: Arc<dyn tenant::TenantManager>,
        config: ServerConfig,
    ) -> Self {
        Self {
            tenant_manager,
            config,
            rate_limiter: rate_limit::RateLimiter::new(),
        }
    }
}

pub fn build_router(state: Arc<AppState>) -> Router {
    let api_routes = Router::new()
        .route("/sessions", post(sessions::create).get(sessions::list))
        .route(
            "/sessions/{id}",
            get(sessions::get)
                .patch(sessions::update)
                .delete(sessions::delete),
        )
        .route("/sessions/{id}/messages", post(messages::send))
        .route("/sessions/{id}/messages/current", delete(messages::interrupt))
        .route("/sessions/{id}/events", get(events::stream))
        .route("/sessions/{id}/compact", post(sessions::compact))
        .route("/sessions/{id}/messages", get(sessions::messages))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit::rate_limit_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::auth_middleware,
        ));

    Router::new()
        .route("/healthz", get(health::get))
        .nest("/api/v1", api_routes)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

pub async fn serve(
    state: Arc<AppState>,
) -> Result<(), Box<dyn std::error::Error>> {
    // 安全启动检查：禁止以默认测试密钥运行
    if state.config.is_default_secret() {
        panic!(
            "Default auth secret detected. Set PANDARIA_AUTH_SECRET environment variable."
        );
    }

    let listener = tokio::net::TcpListener::bind(&state.config.bind_addr).await?;
    let router = build_router(state);

    tracing::info!("api-gateway listening on {}", listener.local_addr()?);

    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
            tracing::info!("shutdown signal received");
        })
        .await?;

    Ok(())
}
