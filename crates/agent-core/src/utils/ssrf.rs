//! SSRF (Server-Side Request Forgery) protection utilities.
//!
//! Provides [`SsrfPolicy`] and [`is_internal_endpoint`] to guard against requests
//! to internal networks and cloud metadata services from the `HttpProxyTool`
//! and webhook delivery paths.
//!
//! ## Allowlist mode
//!
//! By default (`PANDARIA_SSRF_ALLOWLIST` unset), all RFC1918 private ranges,
//! loopback, link-local, and `localhost` are blocked — strict deny.
//!
//! When `PANDARIA_SSRF_ALLOWLIST` is set, listed CIDR ranges and domain
//! suffixes are explicitly allowed. This is required for service-to-service
//! integrations (e.g. pandaria ↔ DayPaw on the same private network).
//!
//! ## Allowlist format
//!
//! ```text
//! PANDARIA_SSRF_ALLOWLIST=10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,daypaw.internal,localhost
//! ```
//!
//! - IPv4 CIDR: `a.b.c.d/n` where `0 <= n <= 32`
//! - IPv6 CIDR: `ipv6-address/n` where `0 <= n <= 128`
//! - Domain suffix: bare hostname, matched by suffix (`daypaw.internal` matches `api.daypaw.internal`)
//! - Multiple entries comma-separated; whitespace trimmed
//!
//! ## Failure modes
//!
//! - Single invalid entry → `tracing::warn!` and skip; other valid entries apply.
//! - Allowlist env var set but **all** entries invalid → [`SsrfPolicy::from_env`] panics,
//!   refusing to start with a half-configured allowlist.
//! - Operator must therefore fix malformed entries before pandaria will boot.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;
use std::sync::Arc;

/// One allowlist rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowedRange {
    CidrV4 {
        network: Ipv4Addr,
        prefix_len: u8,
    },
    CidrV6 {
        network: Ipv6Addr,
        prefix_len: u8,
    },
    /// Domain suffix (case-insensitive). `api.daypaw.internal` matches rule `daypaw.internal`.
    Domain(String),
}

/// Parse failure modes. `AllInvalid` is a higher-level failure surfaced by
/// [`SsrfPolicy::from_csv`] when the operator set the env var but every entry
/// was malformed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SsrfParseError {
    #[default]
    Empty,
    InvalidCidr,
    InvalidDomain,
    /// Set by [`SsrfPolicy::from_csv`] when input was non-empty but zero
    /// entries parsed successfully.
    AllInvalid,
}

/// Parse a single allowlist entry.
///
/// Accepts:
/// - `a.b.c.d/n` — IPv4 CIDR
/// - `ipv6/n` — IPv6 CIDR (heuristic: contains `:`)
/// - `hostname` — domain suffix; single-label allowed (e.g. `localhost`)
///   but a bare IPv4 literal (no `/prefix`) is rejected as InvalidCidr.
pub fn parse_allowed(input: &str) -> Result<AllowedRange, SsrfParseError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(SsrfParseError::Empty);
    }

    // IPv6 CIDR (heuristic: contains colon)
    if s.contains(':') {
        let (addr_str, prefix_str) = s.split_once('/').ok_or(SsrfParseError::InvalidCidr)?;
        let network = Ipv6Addr::from_str(addr_str).map_err(|_| SsrfParseError::InvalidCidr)?;
        let prefix_len: u8 = prefix_str
            .parse()
            .map_err(|_| SsrfParseError::InvalidCidr)?;
        if prefix_len > 128 {
            return Err(SsrfParseError::InvalidCidr);
        }
        return Ok(AllowedRange::CidrV6 {
            network,
            prefix_len,
        });
    }

    // IPv4 CIDR
    if s.contains('/') {
        let (addr_str, prefix_str) = s.split_once('/').ok_or(SsrfParseError::InvalidCidr)?;
        let network = Ipv4Addr::from_str(addr_str).map_err(|_| SsrfParseError::InvalidCidr)?;
        let prefix_len: u8 = prefix_str
            .parse()
            .map_err(|_| SsrfParseError::InvalidCidr)?;
        if prefix_len > 32 {
            return Err(SsrfParseError::InvalidCidr);
        }
        return Ok(AllowedRange::CidrV4 {
            network,
            prefix_len,
        });
    }

    // Bare IPv4 literal without `/prefix` is rejected — must specify a CIDR.
    if Ipv4Addr::from_str(s).is_ok() {
        return Err(SsrfParseError::InvalidCidr);
    }

    // Domain suffix (single-label allowed, e.g. `localhost`)
    if s.contains('/') {
        return Err(SsrfParseError::InvalidDomain);
    }
    Ok(AllowedRange::Domain(s.to_ascii_lowercase()))
}

/// SSRF policy. Constructed once at startup from `PANDARIA_SSRF_ALLOWLIST`,
/// shared via `Arc<SsrfPolicy>` across all `HttpProxyTool` instances and
/// webhook delivery paths.
#[derive(Debug, Clone)]
pub struct SsrfPolicy {
    allowlist: Vec<AllowedRange>,
    /// True when the operator opted into allowlist mode.
    /// When false, `is_internal_endpoint` returns the strict default.
    allowlist_enabled: bool,
}

impl SsrfPolicy {
    /// Strict deny policy. All RFC1918 / loopback / link-local / metadata
    /// addresses are blocked.
    pub fn strict() -> Self {
        Self {
            allowlist: Vec::new(),
            allowlist_enabled: false,
        }
    }

    /// Load policy from `PANDARIA_SSRF_ALLOWLIST` env var.
    /// Empty / unset → strict policy (no allowlist mode).
    /// Parse failures:
    /// - Single invalid entry → warn + skip; remaining valid entries apply
    /// - **All** entries invalid → **panic** (refuse to start)
    pub fn from_env() -> Self {
        match Self::try_from_env() {
            Ok(p) => p,
            Err(SsrfParseError::AllInvalid) => panic!(
                "PANDARIA_SSRF_ALLOWLIST is set but no entries parsed successfully. \
                 Refusing to start. Check the env var for malformed entries \
                 (expected: CIDR like 10.0.0.0/8 or domain like daypaw.internal)."
            ),
            Err(e) => panic!("PANDARIA_SSRF_ALLOWLIST parse error: {:?}", e),
        }
    }

    /// Non-panicking variant of `from_env`. Returns `Ok(strict)` when env unset.
    pub fn try_from_env() -> Result<Self, SsrfParseError> {
        let raw = std::env::var("PANDARIA_SSRF_ALLOWLIST").unwrap_or_default();
        Self::from_csv(&raw)
    }

    /// Parse from a comma-separated string. Empty input → strict policy.
    /// Single invalid entries are skipped (with warn log). All-invalid input
    /// returns [`SsrfParseError::AllInvalid`].
    pub fn from_csv(raw: &str) -> Result<Self, SsrfParseError> {
        let mut allowlist = Vec::new();
        let mut errors: Vec<(String, SsrfParseError)> = Vec::new();
        let mut input_non_empty = false;

        for entry in raw.split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            input_non_empty = true;
            match parse_allowed(entry) {
                Ok(rule) => allowlist.push(rule),
                Err(e) => {
                    tracing::warn!(
                        entry = %entry,
                        error = ?e,
                        "PANDARIA_SSRF_ALLOWLIST: skipping invalid entry"
                    );
                    errors.push((entry.to_string(), e));
                }
            }
        }

        if input_non_empty && allowlist.is_empty() && !errors.is_empty() {
            tracing::error!(
                invalid_entries = ?errors,
                "PANDARIA_SSRF_ALLOWLIST set but no valid entries parsed; refusing"
            );
            return Err(SsrfParseError::AllInvalid);
        }

        tracing::info!(
            entries_parsed = allowlist.len(),
            skipped = errors.len(),
            allowlist_enabled = !allowlist.is_empty(),
            "ssrf policy loaded"
        );

        Ok(Self {
            allowlist_enabled: !allowlist.is_empty(),
            allowlist,
        })
    }

    pub fn allowlist_enabled(&self) -> bool {
        self.allowlist_enabled
    }

    pub fn allowlist_size(&self) -> usize {
        self.allowlist.len()
    }

    /// Check whether `url` is forbidden.
    /// - Returns `false` (allowed) if the URL is in the allowlist.
    /// - Otherwise falls back to the strict deny check.
    pub fn is_internal_endpoint(&self, url: &str) -> bool {
        if self.is_in_allowlist(url) {
            return false;
        }
        is_internal_endpoint_strict(url)
    }

    fn is_in_allowlist(&self, url: &str) -> bool {
        if !self.allowlist_enabled {
            return false;
        }
        let Ok(parsed) = url::Url::parse(url) else {
            return false;
        };
        let host = match parsed.host() {
            Some(h) => h,
            None => return false,
        };
        for rule in &self.allowlist {
            match rule {
                AllowedRange::CidrV4 {
                    network,
                    prefix_len,
                } => {
                    if let url::Host::Ipv4(ip) = host
                        && ipv4_matches(ip, *network, *prefix_len)
                    {
                        return true;
                    }
                }
                AllowedRange::CidrV6 {
                    network,
                    prefix_len,
                } => {
                    if let url::Host::Ipv6(ip) = host
                        && ipv6_matches(ip, *network, *prefix_len)
                    {
                        return true;
                    }
                }
                AllowedRange::Domain(suffix) => {
                    if let url::Host::Domain(name) = host {
                        let name_lower = name.to_ascii_lowercase();
                        if name_lower == *suffix || name_lower.ends_with(&format!(".{}", suffix)) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
}

fn ipv4_matches(ip: Ipv4Addr, network: Ipv4Addr, prefix_len: u8) -> bool {
    if prefix_len == 0 {
        return true;
    }
    if prefix_len > 32 {
        return false;
    }
    let ip_bits = u32::from(ip);
    let net_bits = u32::from(network);
    let mask = if prefix_len == 32 {
        u32::MAX
    } else {
        u32::MAX << (32 - prefix_len)
    };
    (ip_bits & mask) == (net_bits & mask)
}

fn ipv6_matches(ip: Ipv6Addr, network: Ipv6Addr, prefix_len: u8) -> bool {
    if prefix_len == 0 {
        return true;
    }
    if prefix_len > 128 {
        return false;
    }
    let ip_bits = u128::from(ip);
    let net_bits = u128::from(network);
    let mask = if prefix_len == 128 {
        u128::MAX
    } else {
        u128::MAX << (128 - prefix_len)
    };
    (ip_bits & mask) == (net_bits & mask)
}

/// Strict deny check. Blocks RFC1918, loopback, link-local, metadata
/// services, non-http/https schemes, and malformed URLs.
/// Free function retained for backward compatibility (tests, external users
/// that don't need allowlist support).
pub fn is_internal_endpoint(url: &str) -> bool {
    is_internal_endpoint_strict(url)
}

fn is_internal_endpoint_strict(url: &str) -> bool {
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

/// Convenience: `Arc<SsrfPolicy>` for shared ownership across tool instances.
pub type SharedSsrfPolicy = Arc<SsrfPolicy>;

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Existing strict-mode tests (preserved) ─────────────────────────

    #[test]
    fn test_public_urls_allowed() {
        let p = SsrfPolicy::strict();
        let allowed = vec![
            "https://api.example.com/v1/tools",
            "http://example.com:8080/invoke",
            "https://1.1.1.1/dns",
            "https://8.8.8.8/",
            "http://203.0.113.1/",
        ];
        for url in allowed {
            assert!(
                !p.is_internal_endpoint(url),
                "expected {} to be allowed",
                url
            );
        }
    }

    #[test]
    fn test_localhost_blocked() {
        let p = SsrfPolicy::strict();
        let blocked = vec![
            "http://localhost/",
            "http://LOCALHOST/",
            "http://LocalHost:3000/invoke",
            "https://localhost/",
        ];
        for url in blocked {
            assert!(
                p.is_internal_endpoint(url),
                "expected {} to be blocked",
                url
            );
        }
    }

    #[test]
    fn test_loopback_ipv4_blocked() {
        let p = SsrfPolicy::strict();
        let blocked = vec![
            "http://127.0.0.1/",
            "http://127.255.255.255/",
            "https://127.1.2.3:8080/",
        ];
        for url in blocked {
            assert!(
                p.is_internal_endpoint(url),
                "expected {} to be blocked",
                url
            );
        }
    }

    #[test]
    fn test_private_ranges_blocked() {
        let p = SsrfPolicy::strict();
        let blocked = vec![
            "http://10.0.0.1/",
            "http://10.255.255.255/",
            "http://172.16.0.1/",
            "http://172.31.255.255/",
            "http://192.168.1.1/",
            "http://192.168.255.255/",
        ];
        for url in blocked {
            assert!(
                p.is_internal_endpoint(url),
                "expected {} to be blocked",
                url
            );
        }
    }

    #[test]
    fn test_link_local_blocked() {
        let p = SsrfPolicy::strict();
        let blocked = vec![
            "http://169.254.169.254/latest/meta-data/",
            "http://169.254.1.1/",
            "http://169.254.255.255/",
        ];
        for url in blocked {
            assert!(
                p.is_internal_endpoint(url),
                "expected {} to be blocked",
                url
            );
        }
    }

    #[test]
    fn test_zero_network_blocked() {
        let p = SsrfPolicy::strict();
        assert!(p.is_internal_endpoint("http://0.0.0.0/"));
        assert!(p.is_internal_endpoint("http://0.255.255.255/"));
    }

    #[test]
    fn test_ipv6_loopback_blocked() {
        let p = SsrfPolicy::strict();
        let blocked = vec![
            "http://[::1]/",
            "http://[0:0:0:0:0:0:0:1]/",
            "http://[::ffff:127.0.0.1]/",
        ];
        for url in blocked {
            assert!(
                p.is_internal_endpoint(url),
                "expected {} to be blocked",
                url
            );
        }
    }

    #[test]
    fn test_ipv6_ula_blocked() {
        let p = SsrfPolicy::strict();
        assert!(p.is_internal_endpoint("http://[fc00::1]/"));
        assert!(p.is_internal_endpoint("http://[fd00::1]/"));
    }

    #[test]
    fn test_ipv6_link_local_blocked() {
        let p = SsrfPolicy::strict();
        assert!(p.is_internal_endpoint("http://[fe80::1]/"));
        assert!(p.is_internal_endpoint("http://[fe80::ffff]/"));
    }

    #[test]
    fn test_non_http_schemes_blocked() {
        let p = SsrfPolicy::strict();
        assert!(p.is_internal_endpoint("ftp://10.0.0.1/"));
        assert!(p.is_internal_endpoint("file:///etc/passwd"));
        assert!(p.is_internal_endpoint("gopher://127.0.0.1/"));
    }

    #[test]
    fn test_malformed_url_blocked() {
        let p = SsrfPolicy::strict();
        assert!(p.is_internal_endpoint("not-a-url"));
        assert!(p.is_internal_endpoint(""));
        assert!(p.is_internal_endpoint("http://"));
    }

    #[test]
    fn test_ipv4_mapped_private_blocked() {
        let p = SsrfPolicy::strict();
        let blocked = vec![
            "http://[::ffff:10.0.0.1]/",
            "http://[::ffff:192.168.1.1]/",
            "http://[::ffff:172.16.0.1]/",
            "http://[::ffff:169.254.169.254]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:0.0.0.0]/",
        ];
        for url in blocked {
            assert!(
                p.is_internal_endpoint(url),
                "expected {} to be blocked",
                url
            );
        }
    }

    #[test]
    fn test_edge_cases_in_private_ranges() {
        let p = SsrfPolicy::strict();
        // 172.15.x.x is just outside 172.16.0.0/12
        assert!(!p.is_internal_endpoint("http://172.15.255.255/"));
        // 172.32.x.x is just outside 172.16.0.0/12
        assert!(!p.is_internal_endpoint("http://172.32.0.1/"));
        // 192.167.x.x is just outside 192.168.0.0/16
        assert!(!p.is_internal_endpoint("http://192.167.255.255/"));
        // 192.169.x.x is just outside 192.168.0.0/16
        assert!(!p.is_internal_endpoint("http://192.169.0.1/"));
    }

    // ─── Allowlist-mode tests ───────────────────────────────────────────

    #[test]
    fn test_parse_ipv4_cidr() {
        assert_eq!(
            parse_allowed("10.0.0.0/8").unwrap(),
            AllowedRange::CidrV4 {
                network: Ipv4Addr::new(10, 0, 0, 0),
                prefix_len: 8,
            }
        );
    }

    #[test]
    fn test_parse_ipv6_cidr() {
        assert_eq!(
            parse_allowed("fc00::/7").unwrap(),
            AllowedRange::CidrV6 {
                network: Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0),
                prefix_len: 7,
            }
        );
    }

    #[test]
    fn test_parse_domain() {
        assert_eq!(
            parse_allowed("daypaw.internal").unwrap(),
            AllowedRange::Domain("daypaw.internal".into()),
        );
    }

    #[test]
    fn test_parse_rejects_bad_inputs() {
        assert!(parse_allowed("").is_err());
        // Note: single-label hostnames are now valid (e.g. "localhost").
        assert!(parse_allowed("10.0.0.0").is_err()); // bare IPv4 without /prefix
        assert!(parse_allowed("10.0.0.0/33").is_err()); // invalid prefix
        assert!(parse_allowed("10.0.0.0/8/extra").is_err()); // malformed
        assert!(parse_allowed("host/path").is_err()); // domain can't contain /
        assert!(parse_allowed("foo:bar").is_err()); // not a valid IPv6
    }

    #[test]
    fn test_parse_single_label_allowed() {
        assert_eq!(
            parse_allowed("localhost").unwrap(),
            AllowedRange::Domain("localhost".into())
        );
        assert_eq!(
            parse_allowed("internal").unwrap(),
            AllowedRange::Domain("internal".into())
        );
    }

    #[test]
    fn test_from_csv_skips_invalid_keeps_valid() {
        let p = SsrfPolicy::from_csv("10.0.0.0/8,host/path,daypaw.internal").unwrap();
        assert!(p.allowlist_enabled());
        assert_eq!(p.allowlist_size(), 2);
    }

    #[test]
    fn test_from_csv_empty_is_strict_ok() {
        let p = SsrfPolicy::from_csv("").unwrap();
        assert!(!p.allowlist_enabled());
        assert_eq!(p.allowlist_size(), 0);
    }

    #[test]
    fn test_from_csv_whitespace_only_is_strict_ok() {
        let p = SsrfPolicy::from_csv(" , , ").unwrap();
        assert!(!p.allowlist_enabled());
    }

    #[test]
    fn test_from_csv_all_invalid_errors() {
        let res = SsrfPolicy::from_csv("10.0.0.0/33,host/path,foo:bar");
        assert!(matches!(res, Err(SsrfParseError::AllInvalid)));
    }

    #[test]
    fn test_ipv4_cidr_match() {
        assert!(ipv4_matches(
            Ipv4Addr::new(10, 1, 2, 3),
            Ipv4Addr::new(10, 0, 0, 0),
            8
        ));
        assert!(!ipv4_matches(
            Ipv4Addr::new(11, 1, 2, 3),
            Ipv4Addr::new(10, 0, 0, 0),
            8
        ));
        assert!(ipv4_matches(
            Ipv4Addr::new(192, 168, 1, 5),
            Ipv4Addr::new(192, 168, 1, 0),
            24
        ));
        assert!(!ipv4_matches(
            Ipv4Addr::new(192, 168, 2, 5),
            Ipv4Addr::new(192, 168, 1, 0),
            24
        ));
    }

    #[test]
    fn test_allowlist_overrides_strict() {
        // localhost blocked by strict, but allowed by allowlist
        let p = SsrfPolicy::from_csv("localhost").unwrap();
        assert!(!p.is_internal_endpoint("http://localhost:3001/api/tools/x"));
    }

    #[test]
    fn test_allowlist_cidr_overrides_strict() {
        let p = SsrfPolicy::from_csv("10.0.0.0/8").unwrap();
        assert!(!p.is_internal_endpoint("http://10.1.2.3:3001/api/tools/x"));
        // outside the allowlist → strict deny still applies
        assert!(p.is_internal_endpoint("http://192.168.1.1/"));
    }

    #[test]
    fn test_allowlist_domain_suffix() {
        let p = SsrfPolicy::from_csv("daypaw.internal").unwrap();
        assert!(!p.is_internal_endpoint("http://api.daypaw.internal/tools"));
        assert!(!p.is_internal_endpoint("http://daypaw.internal/"));
        // Private IP not in allowlist → strict deny blocks
        assert!(p.is_internal_endpoint("http://192.168.1.1/"));
        // Substring attack (`daypaw.internal.evil.com`) does not match suffix
        // rule (it doesn't end with `.daypaw.internal`). The URL is treated as
        // a public domain by strict mode. Allowing/blocking here is delegated
        // to network-layer DNS / ACL controls — out of scope for SSRF.
    }

    #[test]
    fn test_combined_rules() {
        let p = SsrfPolicy::from_csv("10.0.0.0/8,192.168.0.0/16,daypaw.internal").unwrap();
        assert!(!p.is_internal_endpoint("http://10.1.1.1/"));
        assert!(!p.is_internal_endpoint("http://192.168.1.1/"));
        assert!(!p.is_internal_endpoint("http://api.daypaw.internal/"));
        assert!(p.is_internal_endpoint("http://172.16.0.1/")); // not in allowlist
        assert!(!p.is_internal_endpoint("https://api.example.com/")); // public still allowed
    }

    #[test]
    fn test_domain_case_insensitive() {
        let p = SsrfPolicy::from_csv("DAYPAW.INTERNAL").unwrap();
        assert!(!p.is_internal_endpoint("http://api.daypaw.internal/"));
        assert!(!p.is_internal_endpoint("http://API.DAYPAW.INTERNAL/"));
    }
}
