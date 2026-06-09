use secrecy::{ExposeSecret, SecretString};
use std::net::SocketAddr;

/// 默认测试密钥。生产环境禁止直接使用此值运行。
const DEFAULT_TEST_SECRET: &str = "test-secret-32-chars-long!!!";

/// Gateway 服务器配置。
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind_addr: SocketAddr,
    pub auth_secret: SecretString,
    pub rate_limit: RateLimitConfig,
    pub default_model: String,
    pub default_context_window: u64,
    /// 是否使用完全开放的 CORS（`CorsLayer::permissive()`）。
    /// 生产环境应设为 `false`（默认）。
    pub cors_permissive: bool,
    /// 最大请求体大小（字节），默认 1 MiB。
    pub max_request_body_size: usize,
    /// CORS 允许的来源列表。若设置，覆盖 `cors_permissive`。
    pub cors_origins: Option<Vec<String>>,
}

/// 限流配置。
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub requests_per_second: u32,
    pub burst_size: u32,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([0, 0, 0, 0], 8080)),
            auth_secret: SecretString::from(DEFAULT_TEST_SECRET),
            rate_limit: RateLimitConfig::default(),
            default_model: "claude-sonnet-4".to_string(),
            default_context_window: 128_000,
            cors_permissive: false,
            max_request_body_size: 1024 * 1024,
            cors_origins: None,
        }
    }
}

impl ServerConfig {
    /// 从环境变量加载配置，未设置则使用默认值。
    ///
    /// 读取的环境变量：
    /// - `PANDARIA_BIND_ADDR` — 绑定地址，默认 `0.0.0.0:8080`
    /// - `PANDARIA_AUTH_SECRET` — HMAC 密钥，默认测试密钥（生产环境必须覆盖）
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(addr) = std::env::var("PANDARIA_BIND_ADDR") {
            config.bind_addr = addr
                .parse()
                .unwrap_or_else(|_| panic!("PANDARIA_BIND_ADDR '{}' is invalid", addr));
        }

        if let Ok(secret) = std::env::var("PANDARIA_AUTH_SECRET") {
            config.auth_secret = SecretString::from(secret);
        }

        if let Ok(rps) = std::env::var("PANDARIA_RATE_LIMIT_RPS") {
            config.rate_limit.requests_per_second = rps
                .parse()
                .expect("PANDARIA_RATE_LIMIT_RPS must be a valid u32");
        }
        if let Ok(burst) = std::env::var("PANDARIA_RATE_LIMIT_BURST") {
            config.rate_limit.burst_size = burst
                .parse()
                .expect("PANDARIA_RATE_LIMIT_BURST must be a valid u32");
        }

        if let Ok(origins) = std::env::var("PANDARIA_CORS_ORIGINS") {
            config.cors_origins = Some(origins.split(',').map(|s| s.trim().to_string()).collect());
        }

        config
    }

    /// 检查是否使用了默认测试密钥。
    pub fn is_default_secret(&self) -> bool {
        self.auth_secret.expose_secret() == DEFAULT_TEST_SECRET
    }
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_second: 5,
            burst_size: 10,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ServerConfig::default();
        assert_eq!(config.bind_addr.port(), 8080);
        assert!(config.is_default_secret());
    }

    #[test]
    fn test_from_env_custom_secret() {
        // 直接测试 ServerConfig 构造，避免修改全局环境变量导致并行测试 flaky
        let mut config = ServerConfig::default();
        config.bind_addr = "127.0.0.1:9090".parse().unwrap();
        config.auth_secret = secrecy::SecretString::from("custom-secret-32-chars-long!!");
        assert_eq!(config.bind_addr.to_string(), "127.0.0.1:9090");
        assert!(!config.is_default_secret());
    }
}
