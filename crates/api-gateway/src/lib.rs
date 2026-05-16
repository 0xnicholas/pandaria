//! api-gateway: pandaria 服务端 HTTP 入口层。
//!
//! 提供 REST API + SSE 事件流，对接 TUI 客户端。
//! 职责：认证、路由、SSE 事件转发、限流。
//! 不负责 session 生命周期管理、agent loop 执行、租户调度。

pub mod config;
pub mod error;
pub mod middleware;
pub mod routes;
pub mod server;
pub mod sse;
pub mod types;

pub use config::{RateLimitConfig, ServerConfig};
pub use server::{serve, AppState, build_router};
pub use types::{
    ApiError, CreateSessionRequest, ErrorBody, SendMessageRequest, SendMessageResponse,
    ServerEvent, SessionInfo, UpdateSessionRequest, UsageInfo,
};
