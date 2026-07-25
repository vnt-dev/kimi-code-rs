use std::{collections::HashMap, path::Path};

use crate::_base::di::instantiation::ServiceIdentifier;

use super::contract::LogLevel;

pub const DEFAULT_LOG_LEVEL: LogLevel = LogLevel::Info;
pub const DEFAULT_GLOBAL_MAX_BYTES: u64 = 6 * 1024 * 1024;
pub const DEFAULT_GLOBAL_FILES: usize = 5;
pub const DEFAULT_SESSION_MAX_BYTES: u64 = 5 * 1024 * 1024;
pub const DEFAULT_SESSION_FILES: usize = 3;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoggingConfig {
    pub level: LogLevel,
    pub global_log_path: String,
    pub global_max_bytes: u64,
    pub global_files: usize,
    pub session_max_bytes: u64,
    pub session_files: usize,
}

pub const LOG_OPTIONS_ID: ServiceIdentifier<LoggingConfig> = ServiceIdentifier::new("logOptions");

pub fn resolve_global_log_path(home_dir: impl AsRef<Path>) -> String {
    home_dir
        .as_ref()
        .join("logs")
        .join("kimi-code.log")
        .to_string_lossy()
        .into_owned()
}

pub fn resolve_session_log_path(session_dir: impl AsRef<Path>) -> String {
    session_dir
        .as_ref()
        .join("logs")
        .join("kimi-code.log")
        .to_string_lossy()
        .into_owned()
}

// Original: packages/agent-core-v2/src/_base/log/logConfig.ts, resolveLoggingConfig().
pub fn resolve_logging_config(
    home_dir: impl AsRef<Path>,
    env: &HashMap<String, String>,
) -> LoggingConfig {
    LoggingConfig {
        level: env
            .get("KIMI_LOG_LEVEL")
            .and_then(|value| parse_level(value))
            .unwrap_or(DEFAULT_LOG_LEVEL),
        global_log_path: resolve_global_log_path(home_dir),
        global_max_bytes: env
            .get("KIMI_LOG_GLOBAL_MAX_BYTES")
            .and_then(|value| parse_positive_u64(value))
            .unwrap_or(DEFAULT_GLOBAL_MAX_BYTES),
        global_files: env
            .get("KIMI_LOG_GLOBAL_FILES")
            .and_then(|value| parse_positive_usize(value))
            .unwrap_or(DEFAULT_GLOBAL_FILES),
        session_max_bytes: env
            .get("KIMI_LOG_SESSION_MAX_BYTES")
            .and_then(|value| parse_positive_u64(value))
            .unwrap_or(DEFAULT_SESSION_MAX_BYTES),
        session_files: env
            .get("KIMI_LOG_SESSION_FILES")
            .and_then(|value| parse_positive_usize(value))
            .unwrap_or(DEFAULT_SESSION_FILES),
    }
}

fn parse_level(value: &str) -> Option<LogLevel> {
    match value.trim().to_ascii_lowercase().as_str() {
        "off" => Some(LogLevel::Off),
        "error" => Some(LogLevel::Error),
        "warn" => Some(LogLevel::Warn),
        "info" => Some(LogLevel::Info),
        "debug" => Some(LogLevel::Debug),
        _ => None,
    }
}

fn integer_prefix(value: &str) -> Option<&str> {
    let value = value.trim_start();
    let value = value.strip_prefix('+').unwrap_or(value);
    let end = value
        .char_indices()
        .take_while(|(_, character)| character.is_ascii_digit())
        .map(|(index, character)| index + character.len_utf8())
        .last()?;
    Some(&value[..end])
}

fn parse_positive_u64(value: &str) -> Option<u64> {
    let value = integer_prefix(value)?.parse().ok()?;
    (value > 0).then_some(value)
}

fn parse_positive_usize(value: &str) -> Option<usize> {
    let value = integer_prefix(value)?.parse().ok()?;
    (value > 0).then_some(value)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn config_uses_defaults_and_javascript_integer_prefixes() {
        let defaults = resolve_logging_config("/home/kimi", &HashMap::new());
        assert_eq!(defaults.level, LogLevel::Info);
        assert_eq!(
            PathBuf::from(defaults.global_log_path),
            Path::new("/home/kimi").join("logs").join("kimi-code.log")
        );
        let env = HashMap::from([
            ("KIMI_LOG_LEVEL".into(), " DEBUG ".into()),
            ("KIMI_LOG_GLOBAL_MAX_BYTES".into(), "2048px".into()),
            ("KIMI_LOG_GLOBAL_FILES".into(), "0".into()),
        ]);
        let config = resolve_logging_config("/tmp", &env);
        assert_eq!(config.level, LogLevel::Debug);
        assert_eq!(config.global_max_bytes, 2048);
        assert_eq!(config.global_files, DEFAULT_GLOBAL_FILES);
    }
}
