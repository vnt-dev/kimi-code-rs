use std::collections::HashMap;
use std::net::IpAddr;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostCheckOptions {
    pub bound_host: Option<String>,
    pub extra: Vec<String>,
    pub disable: bool,
}

// Original: packages/kap-server/src/middleware/hostnames.ts
pub fn parse_allowed_hosts(env: &HashMap<String, String>) -> Vec<String> {
    parse_list_env(env.get("KIMI_CODE_ALLOWED_HOSTS"))
}

pub fn is_host_check_disabled(env: &HashMap<String, String>) -> bool {
    env.get("KIMI_CODE_DISABLE_HOST_CHECK")
        .is_some_and(|value| value == "1")
}

pub fn strip_port(host: &str) -> String {
    if host.starts_with('[') {
        return host
            .find(']')
            .map_or(host, |end| &host[..=end])
            .to_lowercase();
    }

    let Some(first_colon) = host.find(':') else {
        return host.to_lowercase();
    };
    if Some(first_colon) == host.rfind(':') {
        let after = &host[first_colon + 1..];
        if !after.is_empty() && after.bytes().all(|byte| byte.is_ascii_digit()) {
            return host[..first_colon].to_lowercase();
        }
    }
    host.to_lowercase()
}

pub fn format_host_error_message(host: Option<&str>) -> String {
    let normalized = host.filter(|value| !value.is_empty()).map(strip_port);
    let host_label = normalized.as_deref().unwrap_or("<missing>");
    let host_arg = normalized.as_deref().unwrap_or("<host>");
    format!(
        "Invalid Host header: {host_label}; allow this host with \
         KIMI_CODE_ALLOWED_HOSTS={host_arg} or 'kimi web --allowed-host {host_arg}'."
    )
}

pub fn is_allowed_host(host: Option<&str>, options: &HostCheckOptions) -> bool {
    if options.disable {
        return true;
    }
    let Some(host) = host.filter(|value| !value.is_empty()) else {
        return false;
    };
    let host = strip_port(host);

    if matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1" | "[::1]")
        || host.ends_with(".localhost")
        || host.parse::<IpAddr>().is_ok()
    {
        return true;
    }
    if options
        .bound_host
        .as_deref()
        .is_some_and(|bound| host == strip_port(bound))
    {
        return true;
    }

    options.extra.iter().any(|entry| {
        if let Some(base) = entry.strip_prefix('.') {
            host == base || host.ends_with(entry)
        } else {
            host == *entry
        }
    })
}

pub(crate) fn parse_list_env(raw: Option<&String>) -> Vec<String> {
    raw.map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_owned)
            .collect()
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ports_and_normalizes_case() {
        assert_eq!(strip_port("localhost:80"), "localhost");
        assert_eq!(strip_port("[::1]:80"), "[::1]");
        assert_eq!(strip_port("1.2.3.4:5678"), "1.2.3.4");
        assert_eq!(strip_port("LOCALHOST"), "localhost");
        assert_eq!(strip_port("::1"), "::1");
    }

    #[test]
    fn applies_default_bound_and_extra_allow_sets() {
        for host in [
            "localhost",
            "localhost:80",
            "foo.localhost",
            "127.0.0.1",
            "127.0.0.1:58627",
            "[::1]",
            "::1",
            "8.8.8.8",
        ] {
            assert!(is_allowed_host(Some(host), &HostCheckOptions::default()));
        }
        for host in ["evil.com", "evil.com:80", "127.0.0.1.evil.com"] {
            assert!(!is_allowed_host(Some(host), &HostCheckOptions::default()));
        }

        let options = HostCheckOptions {
            bound_host: Some("myhost:8080".into()),
            extra: vec![".example.com".into(), "foo".into()],
            disable: false,
        };
        assert!(is_allowed_host(Some("myhost:1234"), &options));
        assert!(is_allowed_host(Some("a.example.com"), &options));
        assert!(is_allowed_host(Some("example.com"), &options));
        assert!(is_allowed_host(Some("foo"), &options));
        assert!(!is_allowed_host(Some("baddexample.com"), &options));
    }

    #[test]
    fn formats_error_guidance() {
        assert_eq!(
            format_host_error_message(Some("APP.Example.com:443")),
            "Invalid Host header: app.example.com; allow this host with \
             KIMI_CODE_ALLOWED_HOSTS=app.example.com or 'kimi web --allowed-host app.example.com'."
        );
    }
}
