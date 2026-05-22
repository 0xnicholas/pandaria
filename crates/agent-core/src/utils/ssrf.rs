//! SSRF (Server-Side Request Forgery) protection utilities.
//!
//! Provides `is_internal_endpoint` to guard against requests to internal
//! networks and cloud metadata services from the `HttpProxyTool` and
//! webhook delivery paths.

use std::net::IpAddr;

/// Check whether a URL points to an internal/reserved address or metadata
/// service that should be blocked.
///
/// Returns `true` if the endpoint is forbidden (hits the SSRF blacklist).
///
/// # Checks performed
/// - Scheme must be `http` or `https`.
/// - Host `localhost` (any case) is blocked.
/// - IPv4 private ranges: `127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`,
///   `192.168.0.0/16`, `169.254.0.0/16` (link-local, includes AWS metadata
///   `169.254.169.254`), `0.0.0.0/8`.
/// - IPv6 loopback `::1`, IPv4-mapped loopback, ULA `fc00::/7`, link-local
///   `fe80::/10`.
/// - Unparseable URLs are treated as forbidden.
pub fn is_internal_endpoint(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        // Malformed URL → forbid.
        return true;
    };

    // Only http/https are allowed for external tools/webhooks.
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return true;
    }

    match parsed.host() {
        Some(url::Host::Domain(name)) => {
            // Block localhost by name.
            if name.eq_ignore_ascii_case("localhost") {
                return true;
            }
            // For other domain names we do **not** perform DNS resolution
            // here (that would itself be a potential SSRF vector).
            false
        }
        Some(url::Host::Ipv4(ip)) => is_internal_ip(&IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => is_internal_ip(&IpAddr::V6(ip)),
        None => true,
    }
}

fn is_internal_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            // 127.0.0.0/8
            if o[0] == 127 {
                return true;
            }
            // 10.0.0.0/8
            if o[0] == 10 {
                return true;
            }
            // 172.16.0.0/12
            if o[0] == 172 && (16..=31).contains(&o[1]) {
                return true;
            }
            // 192.168.0.0/16
            if o[0] == 192 && o[1] == 168 {
                return true;
            }
            // 169.254.0.0/16 (link-local; includes 169.254.169.254)
            if o[0] == 169 && o[1] == 254 {
                return true;
            }
            // 0.0.0.0/8
            if o[0] == 0 {
                return true;
            }
            false
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            // ::1
            if s == [0, 0, 0, 0, 0, 0, 0, 1] {
                return true;
            }
            // ::ffff:<IPv4> (IPv4-mapped addresses) — delegate to IPv4 checks
            if s[0..5] == [0, 0, 0, 0, 0] && s[5] == 0xffff {
                let octets = [
                    (s[6] >> 8) as u8,
                    (s[6] & 0xff) as u8,
                    (s[7] >> 8) as u8,
                    (s[7] & 0xff) as u8,
                ];
                let v4 = std::net::Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3]);
                return is_internal_ip(&std::net::IpAddr::V4(v4));
            }
            // fc00::/7 (ULA)
            if (s[0] & 0xfe00) == 0xfc00 {
                return true;
            }
            // fe80::/10 (link-local)
            if (s[0] & 0xffc0) == 0xfe80 {
                return true;
            }
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_public_urls_allowed() {
        let allowed = vec![
            "https://api.example.com/v1/tools",
            "http://example.com:8080/invoke",
            "https://1.1.1.1/dns",
            "https://8.8.8.8/",
            "http://203.0.113.1/",
        ];
        for url in allowed {
            assert!(!is_internal_endpoint(url), "expected {} to be allowed", url);
        }
    }

    #[test]
    fn test_localhost_blocked() {
        let blocked = vec![
            "http://localhost/",
            "http://LOCALHOST/",
            "http://LocalHost:3000/invoke",
            "https://localhost/",
        ];
        for url in blocked {
            assert!(is_internal_endpoint(url), "expected {} to be blocked", url);
        }
    }

    #[test]
    fn test_loopback_ipv4_blocked() {
        let blocked = vec![
            "http://127.0.0.1/",
            "http://127.255.255.255/",
            "https://127.1.2.3:8080/",
        ];
        for url in blocked {
            assert!(is_internal_endpoint(url), "expected {} to be blocked", url);
        }
    }

    #[test]
    fn test_private_ranges_blocked() {
        let blocked = vec![
            "http://10.0.0.1/",
            "http://10.255.255.255/",
            "http://172.16.0.1/",
            "http://172.31.255.255/",
            "http://192.168.1.1/",
            "http://192.168.255.255/",
        ];
        for url in blocked {
            assert!(is_internal_endpoint(url), "expected {} to be blocked", url);
        }
    }

    #[test]
    fn test_link_local_blocked() {
        let blocked = vec![
            "http://169.254.169.254/latest/meta-data/",
            "http://169.254.1.1/",
            "http://169.254.255.255/",
        ];
        for url in blocked {
            assert!(is_internal_endpoint(url), "expected {} to be blocked", url);
        }
    }

    #[test]
    fn test_zero_network_blocked() {
        assert!(is_internal_endpoint("http://0.0.0.0/"));
        assert!(is_internal_endpoint("http://0.255.255.255/"));
    }

    #[test]
    fn test_ipv6_loopback_blocked() {
        let blocked = vec![
            "http://[::1]/",
            "http://[0:0:0:0:0:0:0:1]/",
            "http://[::ffff:127.0.0.1]/",
        ];
        for url in blocked {
            assert!(is_internal_endpoint(url), "expected {} to be blocked", url);
        }
    }

    #[test]
    fn test_ipv6_ula_blocked() {
        assert!(is_internal_endpoint("http://[fc00::1]/"));
        assert!(is_internal_endpoint("http://[fd00::1]/"));
    }

    #[test]
    fn test_ipv6_link_local_blocked() {
        assert!(is_internal_endpoint("http://[fe80::1]/"));
        assert!(is_internal_endpoint("http://[fe80::ffff]/"));
    }

    #[test]
    fn test_non_http_schemes_blocked() {
        assert!(is_internal_endpoint("ftp://10.0.0.1/"));
        assert!(is_internal_endpoint("file:///etc/passwd"));
        assert!(is_internal_endpoint("gopher://127.0.0.1/"));
    }

    #[test]
    fn test_malformed_url_blocked() {
        assert!(is_internal_endpoint("not-a-url"));
        assert!(is_internal_endpoint(""));
        assert!(is_internal_endpoint("http://"));
    }

    #[test]
    fn test_ipv4_mapped_private_blocked() {
        let blocked = vec![
            "http://[::ffff:10.0.0.1]/",
            "http://[::ffff:192.168.1.1]/",
            "http://[::ffff:172.16.0.1]/",
            "http://[::ffff:169.254.169.254]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:0.0.0.0]/",
        ];
        for url in blocked {
            assert!(is_internal_endpoint(url), "expected {} to be blocked", url);
        }
    }

    #[test]
    fn test_edge_cases_in_private_ranges() {
        // 172.15.x.x is just outside 172.16.0.0/12
        assert!(!is_internal_endpoint("http://172.15.255.255/"));
        // 172.32.x.x is just outside 172.16.0.0/12
        assert!(!is_internal_endpoint("http://172.32.0.1/"));
        // 192.167.x.x is just outside 192.168.0.0/16
        assert!(!is_internal_endpoint("http://192.167.255.255/"));
        // 192.169.x.x is just outside 192.168.0.0/16
        assert!(!is_internal_endpoint("http://192.169.0.1/"));
    }
}
