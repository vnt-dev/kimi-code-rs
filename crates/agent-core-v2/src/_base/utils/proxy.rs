//! Proxy environment resolution shared by HTTP clients and child processes.
//!
//! Original: `packages/agent-core-v2/src/_base/utils/proxy.ts`.

use std::collections::HashMap;

use percent_encoding::percent_decode_str;
use url::Url;

pub type Env = HashMap<String, String>;

const LOOPBACK_NO_PROXY: &[&str] = &["localhost", "127.0.0.1", "::1", "[::1]"];
const SOCKS_SCHEMES: &[&str] = &["socks", "socks4", "socks4a", "socks5", "socks5h"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SocksProxyConfig {
    pub proxy_type: u8,
    pub host: String,
    pub port: u16,
    pub user_id: Option<String>,
    pub password: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProxyDispatcherConfig {
    Http {
        http_proxy: String,
        https_proxy: String,
        no_proxy: String,
    },
    Socks {
        proxy: SocksProxyConfig,
        no_proxy: String,
    },
}

fn scheme_of(value: &str) -> Option<String> {
    let (scheme, _) = value.split_once(':')?;
    let mut characters = scheme.chars();
    let first = characters.next()?;
    (first.is_ascii_alphabetic()
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '.' | '-')
        }))
    .then(|| scheme.to_ascii_lowercase())
}

fn first_non_blank<'a>(env: &'a Env, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .filter_map(|key| env.get(*key))
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
}

fn http_scheme_value(value: Option<&str>) -> Option<&str> {
    value.filter(|value| {
        !scheme_of(value).is_some_and(|scheme| SOCKS_SCHEMES.contains(&scheme.as_str()))
    })
}

fn has_http_proxy(env: &Env) -> bool {
    [
        first_non_blank(env, &["http_proxy", "HTTP_PROXY"]),
        first_non_blank(env, &["https_proxy", "HTTPS_PROXY"]),
        first_non_blank(env, &["all_proxy", "ALL_PROXY"]),
    ]
    .into_iter()
    .any(|value| http_scheme_value(value).is_some())
}

fn resolve_http_proxy_urls(env: &Env) -> (Option<String>, Option<String>) {
    let all_proxy = http_scheme_value(first_non_blank(env, &["all_proxy", "ALL_PROXY"]));
    let http_proxy = http_scheme_value(first_non_blank(env, &["http_proxy", "HTTP_PROXY"]))
        .or(all_proxy)
        .map(str::to_owned);
    let https_proxy = http_scheme_value(first_non_blank(env, &["https_proxy", "HTTPS_PROXY"]))
        .or(all_proxy)
        .map(str::to_owned);
    (http_proxy, https_proxy)
}

// Original: resolveSocksProxy().
pub fn resolve_socks_proxy(env: &Env) -> Option<SocksProxyConfig> {
    let candidates = [
        first_non_blank(env, &["all_proxy", "ALL_PROXY"]),
        first_non_blank(env, &["https_proxy", "HTTPS_PROXY"]),
        first_non_blank(env, &["http_proxy", "HTTP_PROXY"]),
    ];
    for value in candidates.into_iter().flatten() {
        let Some(scheme) = scheme_of(value) else {
            continue;
        };
        if !SOCKS_SCHEMES.contains(&scheme.as_str()) {
            continue;
        }
        let Ok(url) = Url::parse(value) else {
            continue;
        };
        let Some(host) = url.host_str() else {
            continue;
        };
        let host = host.trim_matches(['[', ']']).to_owned();
        let user_id = decode_url_component(url.username());
        let password = url.password().and_then(decode_url_component);
        return Some(SocksProxyConfig {
            proxy_type: if matches!(scheme.as_str(), "socks4" | "socks4a") {
                4
            } else {
                5
            },
            host,
            port: url.port().unwrap_or(1080),
            user_id,
            password,
        });
    }
    None
}

fn decode_url_component(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| percent_decode_str(value).decode_utf8_lossy().into_owned())
}

pub fn is_proxy_configured(env: &Env) -> bool {
    has_http_proxy(env) || resolve_socks_proxy(env).is_some()
}

// Original: resolveNoProxy(). Loopback hosts are always added unless `*` is present.
pub fn resolve_no_proxy(env: &Env) -> String {
    let raw = first_non_blank(env, &["no_proxy", "NO_PROXY"]).unwrap_or_default();
    let mut hosts = raw
        .split(',')
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if hosts.iter().any(|host| host == "*") {
        return "*".into();
    }
    for loopback in LOOPBACK_NO_PROXY {
        if !hosts.iter().any(|host| host == loopback) {
            hosts.push((*loopback).into());
        }
    }
    hosts.join(",")
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NoProxyEntry {
    host: String,
    port: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoProxyMatcher {
    match_all: bool,
    entries: Vec<NoProxyEntry>,
}

// Original: makeNoProxyMatcher(). Rust uses a callable-style value with `is_match`.
pub fn make_no_proxy_matcher(no_proxy: &str) -> NoProxyMatcher {
    let entries = no_proxy
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    NoProxyMatcher {
        match_all: entries.iter().any(|entry| entry == "*"),
        entries: entries
            .into_iter()
            .filter(|entry| entry != "*")
            .map(|entry| parse_no_proxy_entry(&entry))
            .collect(),
    }
}

impl NoProxyMatcher {
    pub fn is_match(&self, host: &str, port: Option<impl ToString>) -> bool {
        if self.match_all {
            return true;
        }
        let target = host
            .to_ascii_lowercase()
            .trim_matches(['[', ']'])
            .to_owned();
        let target_port = port.map(|port| port.to_string());
        self.entries.iter().any(|entry| {
            (entry.port.is_none() || entry.port == target_port)
                && (target == entry.host || target.ends_with(&format!(".{}", entry.host)))
        })
    }
}

fn parse_no_proxy_entry(entry: &str) -> NoProxyEntry {
    let (mut host, port) = if let Some(bracketed) = entry.strip_prefix('[') {
        bracketed.find(']').map_or((entry, None), |close| {
            let host = &bracketed[..close];
            let rest = &bracketed[close + 1..];
            (host, rest.strip_prefix(':').map(str::to_owned))
        })
    } else if let Some(colon) = entry.find(':') {
        let suffix = &entry[colon + 1..];
        if entry.rfind(':') == Some(colon) && suffix.chars().all(|value| value.is_ascii_digit()) {
            (&entry[..colon], Some(suffix.to_owned()))
        } else {
            (entry, None)
        }
    } else {
        (entry, None)
    };
    host = host
        .strip_prefix("*.")
        .or_else(|| host.strip_prefix('.'))
        .unwrap_or(host);
    NoProxyEntry {
        host: host.to_owned(),
        port,
    }
}

// Original: createProxyDispatcher(). The Node dispatcher is represented as
// configuration data so each Rust HTTP transport can apply it to its own client.
pub fn create_proxy_dispatcher(env: &Env) -> Option<ProxyDispatcherConfig> {
    if has_http_proxy(env) {
        let (http_proxy, https_proxy) = resolve_http_proxy_urls(env);
        return Some(ProxyDispatcherConfig::Http {
            http_proxy: http_proxy.unwrap_or_default(),
            https_proxy: https_proxy.unwrap_or_default(),
            no_proxy: resolve_no_proxy(env),
        });
    }
    resolve_socks_proxy(env).map(|proxy| ProxyDispatcherConfig::Socks {
        proxy,
        no_proxy: resolve_no_proxy(env),
    })
}

pub fn install_global_proxy_dispatcher(
    env: &Env,
    set_global_dispatcher: impl FnOnce(ProxyDispatcherConfig),
) -> bool {
    let Some(dispatcher) = create_proxy_dispatcher(env) else {
        return false;
    };
    set_global_dispatcher(dispatcher);
    true
}

pub fn proxy_env_for_child(env: &Env) -> Env {
    if !has_http_proxy(env) {
        return Env::new();
    }
    let no_proxy = resolve_no_proxy(env);
    let mut result = Env::from([
        ("NODE_USE_ENV_PROXY".into(), "1".into()),
        ("NO_PROXY".into(), no_proxy.clone()),
        ("no_proxy".into(), no_proxy),
    ]);
    let (http_proxy, https_proxy) = resolve_http_proxy_urls(env);
    if let Some(http_proxy) = http_proxy {
        result.insert("HTTP_PROXY".into(), http_proxy.clone());
        result.insert("http_proxy".into(), http_proxy);
    }
    if let Some(https_proxy) = https_proxy {
        result.insert("HTTPS_PROXY".into(), https_proxy.clone());
        result.insert("https_proxy".into(), https_proxy);
    }
    result
}

pub fn reconcile_child_no_proxy(child_env: &mut Env, config_env: Option<&Env>) {
    let Some(config_env) = config_env else {
        return;
    };
    let Some(override_value) = first_non_blank(config_env, &["no_proxy", "NO_PROXY"]) else {
        return;
    };
    let override_env = Env::from([
        ("no_proxy".into(), override_value.into()),
        ("NO_PROXY".into(), override_value.into()),
    ]);
    let no_proxy = resolve_no_proxy(&override_env);
    child_env.insert("NO_PROXY".into(), no_proxy.clone());
    child_env.insert("no_proxy".into(), no_proxy);
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::sync::Arc;

    use super::*;

    fn env(entries: &[(&str, &str)]) -> Env {
        entries
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect()
    }

    #[test]
    fn detects_and_parses_http_and_socks_proxy_configuration() {
        assert!(!is_proxy_configured(&Env::new()));
        assert!(is_proxy_configured(&env(&[(
            "HTTP_PROXY",
            "http://p:3128"
        )])));
        assert!(!is_proxy_configured(&env(&[("HTTP_PROXY", "   ")])));
        assert_eq!(
            resolve_socks_proxy(&env(&[("ALL_PROXY", "socks5://user:pass@127.0.0.1:1080")])),
            Some(SocksProxyConfig {
                proxy_type: 5,
                host: "127.0.0.1".into(),
                port: 1080,
                user_id: Some("user".into()),
                password: Some("pass".into()),
            })
        );
        assert_eq!(
            resolve_socks_proxy(&env(&[("ALL_PROXY", "socks4://10.0.0.1")])),
            Some(SocksProxyConfig {
                proxy_type: 4,
                host: "10.0.0.1".into(),
                port: 1080,
                user_id: None,
                password: None,
            })
        );
    }

    #[test]
    fn no_proxy_adds_loopback_and_matches_domains_ports_and_ipv6() {
        assert_eq!(
            resolve_no_proxy(&Env::new()),
            "localhost,127.0.0.1,::1,[::1]"
        );
        assert_eq!(resolve_no_proxy(&env(&[("NO_PROXY", "*")])), "*");
        let matcher = make_no_proxy_matcher("localhost,.example.com,::1");
        assert!(matcher.is_match("localhost", None::<u16>));
        assert!(matcher.is_match("sub.example.com", None::<u16>));
        assert!(matcher.is_match("[::1]", None::<u16>));
        assert!(!matcher.is_match("other.com", None::<u16>));
        let matcher = make_no_proxy_matcher("api.example.com:443");
        assert!(matcher.is_match("api.example.com", Some(443)));
        assert!(!matcher.is_match("api.example.com", Some(80)));
    }

    #[test]
    fn dispatcher_selection_and_installation_preserve_precedence() {
        let http_env = env(&[("HTTP_PROXY", "http://p:3128"), ("NO_PROXY", "corp")]);
        assert_eq!(
            create_proxy_dispatcher(&http_env),
            Some(ProxyDispatcherConfig::Http {
                http_proxy: "http://p:3128".into(),
                https_proxy: "".into(),
                no_proxy: "corp,localhost,127.0.0.1,::1,[::1]".into(),
            })
        );
        let installed = Arc::new(Mutex::new(None));
        let installed_for_callback = Arc::clone(&installed);
        assert!(install_global_proxy_dispatcher(&http_env, move |config| {
            *installed_for_callback.lock() = Some(config);
        }));
        assert!(installed.lock().is_some());
        assert!(!install_global_proxy_dispatcher(&Env::new(), |_| {}));
    }

    #[test]
    fn child_environment_preserves_http_values_and_reconciles_override() {
        assert!(proxy_env_for_child(&env(&[("ALL_PROXY", "socks5://127.0.0.1:1080")])).is_empty());
        let mut child = proxy_env_for_child(&env(&[
            ("HTTP_PROXY", "http://p:3128"),
            ("NO_PROXY", "corp"),
        ]));
        assert_eq!(
            child.get("HTTP_PROXY").map(String::as_str),
            Some("http://p:3128")
        );
        assert_eq!(
            child.get("NO_PROXY").map(String::as_str),
            Some("corp,localhost,127.0.0.1,::1,[::1]")
        );
        reconcile_child_no_proxy(
            &mut child,
            Some(&env(&[("no_proxy", ""), ("NO_PROXY", "real.corp")])),
        );
        assert_eq!(
            child.get("no_proxy").map(String::as_str),
            Some("real.corp,localhost,127.0.0.1,::1,[::1]")
        );
    }
}
