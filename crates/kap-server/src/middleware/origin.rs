use std::collections::HashMap;

use url::Url;

use super::hostnames::{parse_list_env, strip_port};

// Original: packages/kap-server/src/middleware/origin.ts
pub fn parse_cors_origins(env: &HashMap<String, String>) -> Vec<String> {
    parse_list_env(env.get("KIMI_CODE_CORS_ORIGINS"))
}

pub fn origin_host(origin: Option<&str>) -> Option<String> {
    let url = Url::parse(origin?).ok()?;
    let host = url.host_str()?;
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    match url.port() {
        Some(port) => Some(format!("{host}:{port}")),
        None => Some(host),
    }
}

pub fn is_origin_allowed(origin: Option<&str>, host: Option<&str>, allowed: &[String]) -> bool {
    let Some(origin_host) = origin_host(origin) else {
        return true;
    };
    let origin_host = strip_port(&origin_host);
    if let Some(host) = host {
        let host = strip_port(host);
        if origin_host == host || (is_loopback_host(&origin_host) && is_loopback_host(&host)) {
            return true;
        }
    }
    origin.is_some_and(|origin| allowed.iter().any(|entry| entry == origin))
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "::1" | "[::1]")
        || host.starts_with("127.")
        || host.ends_with(".localhost")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_origin_hosts() {
        assert_eq!(origin_host(Some("https://foo.com")), Some("foo.com".into()));
        assert_eq!(
            origin_host(Some("http://localhost:80")),
            Some("localhost".into())
        );
        assert_eq!(
            origin_host(Some("http://127.0.0.1:58627")),
            Some("127.0.0.1:58627".into())
        );
        assert_eq!(origin_host(Some("not a url")), None);
        assert_eq!(origin_host(None), None);
    }

    #[test]
    fn preserves_origin_policy() {
        assert!(is_origin_allowed(
            Some("http://localhost:80"),
            Some("localhost:80"),
            &[]
        ));
        assert!(!is_origin_allowed(
            Some("http://evil.com"),
            Some("localhost:80"),
            &[]
        ));
        assert!(is_origin_allowed(
            Some("https://foo.com"),
            Some("localhost:80"),
            &["https://foo.com".into()]
        ));
        assert!(is_origin_allowed(None, Some("localhost:80"), &[]));
        assert!(is_origin_allowed(
            Some("not a url"),
            Some("localhost:80"),
            &[]
        ));
        assert!(is_origin_allowed(
            Some("http://localhost:5175"),
            Some("127.0.0.1:58627"),
            &[]
        ));
        assert!(!is_origin_allowed(
            Some("http://localhost:5175"),
            Some("example.com:80"),
            &[]
        ));
    }
}
