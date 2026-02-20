//! Shared utility functions for API handlers.

use axum::http::{header, HeaderMap};

use crate::config::AuthConfig;

/// Extract client IP from request headers using configurable proxy trust.
///
/// Checks headers in order based on `auth_config.trusted_proxy_header`:
/// - `X-Forwarded-For` (default): multi-value, trusts nth-from-last entry per `trusted_proxy_depth`
/// - `X-Real-IP`: single-value (nginx)
/// - `CF-Connecting-IP`: single-value (Cloudflare)
/// - `Forwarded`: RFC 7239 format
///
/// Falls back to `None` if the configured header is absent (caller should use peer IP or "unknown").
pub(crate) fn extract_client_ip(headers: &HeaderMap, auth_config: &AuthConfig) -> Option<String> {
    let header_name = &auth_config.trusted_proxy_header;
    let depth = auth_config.trusted_proxy_depth;

    // depth=0 means trust only peer IP (no proxy headers)
    if depth == 0 {
        return None;
    }

    let header_value = headers.get(header_name.as_str())?;
    let value = header_value.to_str().ok()?;

    if header_name.eq_ignore_ascii_case("X-Forwarded-For") {
        // XFF can contain multiple IPs: client, proxy1, proxy2, ...LB
        // Trust the nth-from-last entry (depth=1 means last entry, which is what the LB appended)
        let ips: Vec<&str> = value.split(',').map(|s| s.trim()).collect();
        let index = ips.len().saturating_sub(depth);
        ips.get(index).map(|s| s.to_string())
    } else if header_name.eq_ignore_ascii_case("Forwarded") {
        // RFC 7239: Forwarded: for=192.0.2.60;proto=http;by=203.0.113.43
        value
            .split(';')
            .find(|part| part.trim().to_lowercase().starts_with("for="))
            .and_then(|part| {
                part.trim().strip_prefix("for=").or_else(|| part.trim().strip_prefix("For="))
            })
            .map(|ip| ip.trim_matches('"').to_string())
    } else {
        // Single-value headers: X-Real-IP, CF-Connecting-IP
        Some(value.trim().to_string())
    }
}

/// Extract User-Agent header from request.
pub(crate) fn extract_user_agent(headers: &HeaderMap) -> Option<String> {
    headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> AuthConfig {
        AuthConfig::default()
    }

    fn config_with_header(header: &str, depth: usize) -> AuthConfig {
        AuthConfig {
            trusted_proxy_header: header.to_string(),
            trusted_proxy_depth: depth,
            ..AuthConfig::default()
        }
    }

    #[test]
    fn test_extract_client_ip_xff_single() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "192.168.1.1".parse().unwrap());

        let ip = extract_client_ip(&headers, &default_config());
        assert_eq!(ip, Some("192.168.1.1".to_string()));
    }

    #[test]
    fn test_extract_client_ip_xff_multiple_depth_1() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "10.0.0.1, 172.16.0.1, 192.168.1.1".parse().unwrap());

        // depth=1 trusts the last entry (appended by LB)
        let ip = extract_client_ip(&headers, &default_config());
        assert_eq!(ip, Some("192.168.1.1".to_string()));
    }

    #[test]
    fn test_extract_client_ip_xff_depth_2() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "10.0.0.1, 172.16.0.1, 192.168.1.1".parse().unwrap());

        // depth=2 trusts the second-from-last entry
        let config = config_with_header("X-Forwarded-For", 2);
        let ip = extract_client_ip(&headers, &config);
        assert_eq!(ip, Some("172.16.0.1".to_string()));
    }

    #[test]
    fn test_extract_client_ip_depth_0_returns_none() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "192.168.1.1".parse().unwrap());

        let config = config_with_header("X-Forwarded-For", 0);
        let ip = extract_client_ip(&headers, &config);
        assert_eq!(ip, None);
    }

    #[test]
    fn test_extract_client_ip_cf_connecting_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("cf-connecting-ip", "203.0.113.50".parse().unwrap());

        let config = config_with_header("CF-Connecting-IP", 1);
        let ip = extract_client_ip(&headers, &config);
        assert_eq!(ip, Some("203.0.113.50".to_string()));
    }

    #[test]
    fn test_extract_client_ip_x_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "198.51.100.10".parse().unwrap());

        let config = config_with_header("X-Real-IP", 1);
        let ip = extract_client_ip(&headers, &config);
        assert_eq!(ip, Some("198.51.100.10".to_string()));
    }

    #[test]
    fn test_extract_client_ip_no_header() {
        let headers = HeaderMap::new();
        let ip = extract_client_ip(&headers, &default_config());
        assert_eq!(ip, None);
    }

    #[test]
    fn test_extract_user_agent() {
        let mut headers = HeaderMap::new();
        headers.insert(header::USER_AGENT, "Mozilla/5.0 (Test)".parse().unwrap());

        let ua = extract_user_agent(&headers);
        assert_eq!(ua, Some("Mozilla/5.0 (Test)".to_string()));
    }

    #[test]
    fn test_extract_user_agent_no_header() {
        let headers = HeaderMap::new();
        let ua = extract_user_agent(&headers);
        assert_eq!(ua, None);
    }
}
