use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use dashmap::DashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::config::RateLimitConfig;
use crate::error::GatewayError;
use crate::middleware::TenantId;
use crate::server::AppState;

struct TokenBucket {
    tokens: f64,
    max_tokens: f64,
    refill_rate: f64,
    last_refill: Instant,
}

/// 限流器：每个租户一个 TokenBucket。
pub struct RateLimiter {
    buckets: DashMap<String, TokenBucket>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            buckets: DashMap::new(),
        }
    }

    pub fn check(&self, tenant_id: &str, config: &RateLimitConfig) -> bool {
        let max_tokens = config.burst_size as f64;
        let refill_rate = config.requests_per_second as f64;

        let mut entry = self.buckets.entry(tenant_id.to_string()).or_insert(TokenBucket {
            tokens: max_tokens,
            max_tokens,
            refill_rate,
            last_refill: Instant::now(),
        });

        let bucket = entry.value_mut();
        let now = Instant::now();
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * bucket.refill_rate).min(bucket.max_tokens);
        bucket.last_refill = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// 限流中间件。必须在 auth 中间件之后执行（依赖 req.extensions 中的 TenantId）。
pub async fn rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, GatewayError> {
    let tenant_id = req
        .extensions()
        .get::<TenantId>()
        .map(|t| t.0.clone())
        .unwrap_or_default();

    if tenant_id.is_empty() {
        return Ok(next.run(req).await);
    }

    if !state.rate_limiter.check(&tenant_id, &state.config.rate_limit) {
        return Err(GatewayError::RateLimited);
    }

    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use super::*;

    #[test]
    fn test_rate_limiter_allows_within_burst() {
        let limiter = RateLimiter::new();
        let config = RateLimitConfig {
            requests_per_second: 5,
            burst_size: 3,
        };

        assert!(limiter.check("t1", &config));
        assert!(limiter.check("t1", &config));
        assert!(limiter.check("t1", &config));
        assert!(!limiter.check("t1", &config)); // exceeded burst
    }

    #[test]
    fn test_rate_limiter_per_tenant_isolation() {
        let limiter = RateLimiter::new();
        let config = RateLimitConfig {
            requests_per_second: 5,
            burst_size: 1,
        };

        assert!(limiter.check("t1", &config));
        assert!(!limiter.check("t1", &config));
        assert!(limiter.check("t2", &config));
    }

    #[test]
    fn test_rate_limiter_refill() {
        let limiter = RateLimiter::new();
        let config = RateLimitConfig {
            requests_per_second: 100, // fast refill
            burst_size: 1,
        };

        assert!(limiter.check("t1", &config));
        assert!(!limiter.check("t1", &config));

        std::thread::sleep(Duration::from_millis(20)); // wait for refill
        assert!(limiter.check("t1", &config));
    }
}
