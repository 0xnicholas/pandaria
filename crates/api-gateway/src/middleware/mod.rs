pub mod auth;
pub mod rate_limit;
pub mod tracing_mw;

/// 注入 request extensions 的 tenant_id newtype，避免与其他 String 扩展冲突。
#[derive(Clone, Debug)]
pub struct TenantId(pub String);
