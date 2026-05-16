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
            auth_secret: SecretString::new(DEFAULT_TEST_SECRET.into()),
            rate_limit: RateLimitConfig::default(),
            default_model: "claude-sonnet-4".to_string(),
            default_context_window: 128_000,
        }
    }
}

impl ServerConfig {
    /// 从环境变量加载配置，未设置则使用默认值。
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(addr) = std::env::var("PANDARIA_BIND_ADDR") {
            if let Ok(parsed) = addr.parse() {
                config.bind_addr = parsed;
            }
        }

        if let Ok(secret) = std::env::var("PANDARIA_AUTH_SECRET") {
            config.auth_secret = SecretString::new(secret.into());
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
    fn test_from_env_override() {
        unsafe {
            std::env::set_var("PANDARIA_BIND_ADDR", "127.0.0.1:9090");
            std::env::set_var("PANDARIA_AUTH_SECRET", "custom-secret-32-chars-long!!");
        }
        let config = ServerConfig::from_env();
        assert_eq!(config.bind_addr.to_string(), "127.0.0.1:9090");
        assert!(!config.is_default_secret());
        unsafe {
            std::env::remove_var("PANDARIA_BIND_ADDR");
            std::env::remove_var("PANDARIA_AUTH_SECRET");
        }
    }
}
