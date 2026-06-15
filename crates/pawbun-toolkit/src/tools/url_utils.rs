//! URL 验证与安全策略工具。
//!
//! 提供 SSRF（服务器端请求伪造）防护和安全的 HTTP Client 构建。

use std::net::IpAddr;

/// 验证 URL 是否安全（防止 SSRF）。
///
/// 拒绝以下地址：
/// - 回环地址（127.0.0.0/8, ::1）
/// - 私有地址（10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, fc00::/7）
/// - 链路本地地址（169.254.0.0/16, fe80::/10）
/// - 未指定地址（0.0.0.0, ::）
/// - localhost 域名
pub fn validate_url(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    let host = parsed.host_str().ok_or("URL missing host")?;

    // 拒绝 localhost 域名
    if host.eq_ignore_ascii_case("localhost") {
        return Err("SSRF protection: localhost is not allowed".into());
    }

    // 去掉 IPv6 的方括号以便解析为 IpAddr
    let ip_candidate = if host.starts_with('[') {
        host.trim_start_matches('[')
            .split(']')
            .next()
            .unwrap_or(host)
    } else {
        host
    };

    // 如果 host 是 IP 地址字面量，检查是否属于受限范围
    if let Ok(ip) = ip_candidate.parse::<IpAddr>() {
        if is_blocked_ip(&ip) {
            return Err(format!("SSRF protection: blocked IP {ip}"));
        }
    }

    Ok(())
}

fn is_blocked_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || is_ipv6_unique_local(v6)
                || is_ipv6_link_local(v6)
        }
    }
}

/// fc00::/7
fn is_ipv6_unique_local(ip: &std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

/// fe80::/10
fn is_ipv6_link_local(ip: &std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

/// 创建带安全策略的 reqwest::Client。
///
/// 配置：
/// - 连接 + 读取超时 30 秒
/// - 最多 10 次重定向
pub fn build_safe_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .no_proxy()
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blocks_localhost() {
        assert!(validate_url("http://localhost/test").is_err());
        assert!(validate_url("http://LOCALHOST:8080/").is_err());
    }

    #[test]
    fn test_blocks_loopback_v4() {
        assert!(validate_url("http://127.0.0.1/test").is_err());
        assert!(validate_url("http://127.255.255.254/").is_err());
    }

    #[test]
    fn test_blocks_private_v4() {
        assert!(validate_url("http://10.0.0.1/test").is_err());
        assert!(validate_url("http://172.16.0.1/test").is_err());
        assert!(validate_url("http://192.168.1.1/test").is_err());
    }

    #[test]
    fn test_blocks_link_local_v4() {
        assert!(validate_url("http://169.254.1.1/test").is_err());
    }

    #[test]
    fn test_blocks_unspecified() {
        assert!(validate_url("http://0.0.0.0/test").is_err());
    }

    #[test]
    fn test_blocks_loopback_v6() {
        assert!(validate_url("http://[::1]/test").is_err());
    }

    #[test]
    fn test_allows_public_ip() {
        assert!(validate_url("http://8.8.8.8/test").is_ok());
        assert!(validate_url("http://1.1.1.1/test").is_ok());
    }

    #[test]
    fn test_allows_public_domain() {
        assert!(validate_url("https://example.com/path").is_ok());
        assert!(validate_url("https://api.github.com/v1").is_ok());
    }

    #[test]
    fn test_rejects_missing_host() {
        assert!(validate_url("file:///etc/passwd").is_err());
    }
}
