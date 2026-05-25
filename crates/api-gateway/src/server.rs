use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware,
    routing::{delete, get, post},
};
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::config::ServerConfig;
use crate::middleware::{auth, rate_limit};
use crate::routes::{events, health, messages, metrics, sessions};

use crate::types::SessionInfo;

/// 应用状态。
pub struct AppState {
    pub tenant_manager: Arc<dyn tenant::TenantManager>,
    pub config: ServerConfig,
    pub rate_limiter: rate_limit::RateLimiter,
}

impl AppState {
    pub fn new(tenant_manager: Arc<dyn tenant::TenantManager>, config: ServerConfig) -> Self {
        Self {
            tenant_manager,
            config,
            rate_limiter: rate_limit::RateLimiter::new(),
        }
    }

    /// Enrich tenant `SessionInfo` with gateway-level defaults.
    pub fn enrich_session_info(&self, info: tenant::SessionInfo) -> SessionInfo {
        let mut s: SessionInfo = info.into();
        s.context_window = Some(self.config.default_context_window);
        s
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
        .route(
            "/sessions/{id}/messages/current",
            delete(messages::interrupt),
        )
        .route("/sessions/{id}/state", get(sessions::get_state))
        .route("/sessions/{id}/clone", post(sessions::clone))
        .route("/sessions/{id}/reset", post(sessions::reset))
        .route("/sessions/{id}/events", get(events::stream))
        .route("/sessions/{id}/ws", get(crate::routes::ws::session_ws))
        .route("/sessions/{id}/compact", post(sessions::compact))
        .route("/sessions/{id}/messages", get(sessions::messages))
        .route("/sessions/batch", post(sessions::batch_create))
        .route("/tenant/quota", get(sessions::get_quota))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit::rate_limit_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::auth_middleware,
        ));

    let cors = if state.config.cors_permissive {
        CorsLayer::permissive()
    } else if let Some(ref origins) = state.config.cors_origins {
        use tower_http::cors::AllowOrigin;
        let allowed: Vec<_> = origins.iter().filter_map(|o| o.parse().ok()).collect();
        CorsLayer::new().allow_origin(AllowOrigin::list(allowed))
    } else {
        CorsLayer::new()
    };

    Router::new()
        .route("/healthz", get(health::get))
        .route("/metrics", get(metrics::get))
        .nest("/api/v1", api_routes)
        .layer(DefaultBodyLimit::max(state.config.max_request_body_size))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}

pub async fn serve(state: Arc<AppState>) -> Result<(), Box<dyn std::error::Error>> {
    // 安全启动检查：禁止以默认测试密钥运行
    if state.config.is_default_secret() {
        panic!("Default auth secret detected. Set PANDARIA_AUTH_SECRET environment variable.");
    }

    let listener = tokio::net::TcpListener::bind(&state.config.bind_addr).await?;
    let router = build_router(state.clone());

    tracing::info!("api-gateway listening on {}", listener.local_addr()?);

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let ctrl_c = async {
                tokio::signal::ctrl_c().await.ok();
            };
            let sigterm = async {
                let mut sigterm =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("SIGTERM handler");
                sigterm.recv().await;
            };
            tokio::select! {
                _ = ctrl_c => {},
                _ = sigterm => {},
            }
            tracing::info!("shutdown signal received, draining sessions...");
            state.tenant_manager.shutdown().await;
            tracing::info!("shutdown complete");
        })
        .await?;

    Ok(())
}
