use std::{collections::BTreeMap, error::Error, fmt, fs, path::Path};

use url::Url;

pub const LOCAL_SERVER_HOST: &str = "127.0.0.1";
pub const DEFAULT_LAN_HOST: &str = "0.0.0.0";
pub const DEFAULT_SERVER_HOST: &str = LOCAL_SERVER_HOST;
pub const DEFAULT_SERVER_PORT: &str = "58627";
pub const DEFAULT_SERVER_PORT_NUMBER: u16 = 58_627;
pub const DEFAULT_SERVER_ORIGIN: &str = "http://127.0.0.1:58627";
pub const SERVER_TOKEN_FILE: &str = "server.token";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerLogLevel {
    Fatal,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
    Silent,
}

impl ServerLogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fatal => "fatal",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
            Self::Silent => "silent",
        }
    }
}

pub const DEFAULT_LOG_LEVEL: ServerLogLevel = ServerLogLevel::Info;
pub const DEFAULT_FOREGROUND_LOG_LEVEL: ServerLogLevel = ServerLogLevel::Silent;
pub const VALID_LOG_LEVELS: &[ServerLogLevel] = &[
    ServerLogLevel::Fatal,
    ServerLogLevel::Error,
    ServerLogLevel::Warn,
    ServerLogLevel::Info,
    ServerLogLevel::Debug,
    ServerLogLevel::Trace,
    ServerLogLevel::Silent,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostInput {
    Missing,
    Bare,
    Value(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCliOptions {
    pub host: HostInput,
    pub port: Option<String>,
    pub log_level: Option<String>,
    pub debug_endpoints: bool,
    pub insecure_no_tls: bool,
    pub allow_remote_shutdown: bool,
    pub allow_remote_terminals: bool,
    pub dangerous_bypass_auth: bool,
    pub allowed_hosts: Vec<String>,
}

impl Default for ServerCliOptions {
    fn default() -> Self {
        Self {
            host: HostInput::Missing,
            port: None,
            log_level: None,
            debug_endpoints: false,
            insecure_no_tls: true,
            allow_remote_shutdown: false,
            allow_remote_terminals: false,
            dangerous_bypass_auth: false,
            allowed_hosts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedServerOptions {
    pub host: String,
    pub port: u16,
    pub log_level: ServerLogLevel,
    pub debug_endpoints: bool,
    pub insecure_no_tls: bool,
    pub allow_remote_shutdown: bool,
    pub allow_remote_terminals: bool,
    pub dangerous_bypass_auth: bool,
    pub allowed_hosts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerOptionError(String);

impl ServerOptionError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ServerOptionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ServerOptionError {}

// Original:
//   apps/kimi-code/src/cli/sub/web/shared.ts
//   parseServerOptions()
pub fn parse_server_options(
    options: &ServerCliOptions,
) -> Result<ParsedServerOptions, ServerOptionError> {
    Ok(ParsedServerOptions {
        host: parse_host(&options.host),
        port: parse_port(
            options.port.as_deref(),
            "--port",
            DEFAULT_SERVER_PORT_NUMBER,
        )?,
        log_level: parse_log_level(Some(
            options
                .log_level
                .as_deref()
                .unwrap_or(DEFAULT_FOREGROUND_LOG_LEVEL.as_str()),
        ))?,
        debug_endpoints: options.debug_endpoints,
        insecure_no_tls: options.insecure_no_tls,
        allow_remote_shutdown: options.allow_remote_shutdown,
        allow_remote_terminals: options.allow_remote_terminals,
        dangerous_bypass_auth: options.dangerous_bypass_auth,
        allowed_hosts: parse_allowed_host_args(&options.allowed_hosts),
    })
}

// Original: parseAllowedHostArgs()
pub fn parse_allowed_host_args(raw: &[String]) -> Vec<String> {
    raw.iter()
        .flat_map(|entry| entry.split(','))
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse_host(raw: &HostInput) -> String {
    match raw {
        HostInput::Missing => DEFAULT_SERVER_HOST.to_owned(),
        HostInput::Bare => DEFAULT_LAN_HOST.to_owned(),
        HostInput::Value(value) if value.is_empty() => DEFAULT_LAN_HOST.to_owned(),
        HostInput::Value(value) => value.clone(),
    }
}

// Original: parsePort()
pub fn parse_port(raw: Option<&str>, label: &str, fallback: u16) -> Result<u16, ServerOptionError> {
    let Some(raw) = raw else {
        return Ok(fallback);
    };
    let Some(value) = parse_javascript_integer(raw) else {
        return Err(ServerOptionError::new(format!(
            "error: invalid {label} value: {raw}"
        )));
    };
    u16::try_from(value)
        .map_err(|_| ServerOptionError::new(format!("error: invalid {label} value: {raw}")))
}

fn parse_javascript_integer(raw: &str) -> Option<i64> {
    let raw = raw.trim_start();
    let (sign, digits) = if let Some(rest) = raw.strip_prefix('-') {
        (-1_i64, rest)
    } else if let Some(rest) = raw.strip_prefix('+') {
        (1_i64, rest)
    } else {
        (1_i64, raw)
    };
    let digit_count = digits.bytes().take_while(u8::is_ascii_digit).count();
    if digit_count == 0 {
        return None;
    }
    digits[..digit_count]
        .parse::<i64>()
        .ok()
        .and_then(|value| value.checked_mul(sign))
}

// Original: parseLogLevel()
pub fn parse_log_level(raw: Option<&str>) -> Result<ServerLogLevel, ServerOptionError> {
    let raw = raw.unwrap_or(DEFAULT_LOG_LEVEL.as_str());
    match raw {
        "fatal" => Ok(ServerLogLevel::Fatal),
        "error" => Ok(ServerLogLevel::Error),
        "warn" => Ok(ServerLogLevel::Warn),
        "info" => Ok(ServerLogLevel::Info),
        "debug" => Ok(ServerLogLevel::Debug),
        "trace" => Ok(ServerLogLevel::Trace),
        "silent" => Ok(ServerLogLevel::Silent),
        raw => Err(ServerOptionError::new(format!(
            "error: invalid --log-level value: {raw} (allowed: fatal, error, warn, info, debug, trace, silent)"
        ))),
    }
}

pub fn default_log_level() -> ServerLogLevel {
    DEFAULT_LOG_LEVEL
}

// Original: serverOrigin()
pub fn server_origin(host: &str, port: u16) -> String {
    format!("http://{host}:{port}")
}

// Original: normalizeServerOrigin()
pub fn normalize_server_origin(value: &str) -> Result<String, url::ParseError> {
    let mut url = Url::parse(value)?;
    let mut path = url.path().to_owned();
    if path.ends_with("/api/v1/") {
        path.truncate(path.len() - "/api/v1/".len());
    } else if path.ends_with("/api/v1") {
        path.truncate(path.len() - "/api/v1".len());
    }
    if path.ends_with('/') {
        path.pop();
    }
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

// Original: resolveServerToken()
pub fn resolve_server_token(home_dir: &Path) -> Result<String, ServerOptionError> {
    let token_path = home_dir.join(SERVER_TOKEN_FILE);
    fs::read_to_string(&token_path)
        .map(|token| token.trim().to_owned())
        .map_err(|_| {
            ServerOptionError::new(format!(
                "unable to read server token at {}; has the server been started at least once?",
                token_path.display()
            ))
        })
}

// Original: tryResolveServerToken()
pub fn try_resolve_server_token(home_dir: &Path) -> Option<String> {
    resolve_server_token(home_dir).ok()
}

// Original: authHeaders()
pub fn auth_headers(token: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("Authorization".to_owned(), format!("Bearer {token}"))])
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    #[test]
    fn parses_server_defaults_and_flags() {
        let parsed = parse_server_options(&ServerCliOptions::default()).expect("defaults");
        assert_eq!(parsed.host, "127.0.0.1");
        assert_eq!(parsed.port, 58_627);
        assert_eq!(parsed.log_level, ServerLogLevel::Silent);
        assert!(parsed.insecure_no_tls);

        let parsed = parse_server_options(&ServerCliOptions {
            host: HostInput::Bare,
            port: Some("8080".to_owned()),
            log_level: Some("debug".to_owned()),
            debug_endpoints: true,
            allowed_hosts: vec![".example.com, app.example.com".to_owned()],
            ..ServerCliOptions::default()
        })
        .expect("custom options");
        assert_eq!(parsed.host, "0.0.0.0");
        assert_eq!(parsed.port, 8_080);
        assert_eq!(parsed.log_level, ServerLogLevel::Debug);
        assert_eq!(parsed.allowed_hosts, [".example.com", "app.example.com"]);
    }

    #[test]
    fn preserves_javascript_port_parsing_and_range_checks() {
        assert_eq!(parse_port(None, "--port", 123).expect("fallback"), 123);
        assert_eq!(parse_port(Some("8080"), "--port", 0).expect("port"), 8_080);
        assert_eq!(
            parse_port(Some("8080junk"), "--port", 0).expect("prefix"),
            8_080
        );
        for raw in ["99999", "-1", "x", ""] {
            assert!(parse_port(Some(raw), "--port", 0).is_err(), "{raw}");
        }
    }

    #[test]
    fn validates_log_levels() {
        assert_eq!(default_log_level(), ServerLogLevel::Info);
        assert_eq!(
            parse_log_level(None).expect("default"),
            ServerLogLevel::Info
        );
        assert_eq!(
            parse_log_level(Some("debug")).expect("debug"),
            ServerLogLevel::Debug
        );
        assert!(parse_log_level(Some("shout")).is_err());
    }

    #[test]
    fn builds_and_normalizes_server_origins() {
        assert_eq!(server_origin("127.0.0.1", 58_627), "http://127.0.0.1:58627");
        assert_eq!(
            normalize_server_origin("http://example.test:1234/api/v1/?q=1#fragment").expect("url"),
            "http://example.test:1234"
        );
        assert_eq!(
            normalize_server_origin("http://example.test/base/").expect("url"),
            "http://example.test/base"
        );
    }

    #[test]
    fn reads_trims_and_best_effort_resolves_server_tokens() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("kimi-server-token-{unique}"));
        fs::create_dir(&directory).expect("create temp directory");
        fs::write(directory.join(SERVER_TOKEN_FILE), "  secret-token  \n").expect("write token");
        assert_eq!(
            resolve_server_token(&directory).expect("token"),
            "secret-token"
        );
        assert_eq!(
            try_resolve_server_token(&directory).as_deref(),
            Some("secret-token")
        );
        fs::remove_file(directory.join(SERVER_TOKEN_FILE)).expect("remove token");
        assert!(
            resolve_server_token(&directory)
                .unwrap_err()
                .to_string()
                .contains("unable to read server token")
        );
        assert_eq!(try_resolve_server_token(&directory), None);
        fs::remove_dir(&directory).expect("remove temp directory");
    }

    #[test]
    fn builds_bearer_authorization_header() {
        assert_eq!(auth_headers("abc")["Authorization"], "Bearer abc");
    }
}
