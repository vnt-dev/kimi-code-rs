use std::collections::HashMap;
use std::time::Duration;

const DEFAULT_TIMEOUT_MS: u64 = 4_000;
const DEFAULT_CACHE_LIMIT: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotReaderMode {
    Auto,
    Legacy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotConfig {
    pub mode: SnapshotReaderMode,
    pub timeout: Duration,
    pub cache_limit: usize,
}

// Original: snapshotConfig.ts, loadSnapshotConfig().
pub fn load_snapshot_config(env: &HashMap<String, String>) -> SnapshotConfig {
    let mode = match env
        .get("KIMI_SNAPSHOT_READER")
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("legacy") => SnapshotReaderMode::Legacy,
        _ => SnapshotReaderMode::Auto,
    };
    SnapshotConfig {
        mode,
        timeout: Duration::from_millis(parse_integer(
            env.get("KIMI_SNAPSHOT_TIMEOUT_MS").map(String::as_str),
            DEFAULT_TIMEOUT_MS,
            100,
        )),
        cache_limit: parse_integer(
            env.get("KIMI_SNAPSHOT_CACHE_LIMIT").map(String::as_str),
            DEFAULT_CACHE_LIMIT as u64,
            1,
        ) as usize,
    }
}

/// Match JavaScript Number.parseInt(value, 10): leading whitespace and sign
/// are accepted, and parsing stops at the first non-decimal character.
fn parse_integer(value: Option<&str>, fallback: u64, minimum: u64) -> u64 {
    let Some(value) = value else {
        return fallback;
    };
    let value = value.trim_start();
    let (negative, digits) = match value.as_bytes().first() {
        Some(b'-') => (true, &value[1..]),
        Some(b'+') => (false, &value[1..]),
        _ => (false, value),
    };
    let length = digits.bytes().take_while(u8::is_ascii_digit).count();
    if negative || length == 0 {
        return fallback;
    }
    digits[..length]
        .parse::<u64>()
        .ok()
        .filter(|value| *value >= minimum)
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_parses_environment() {
        assert_eq!(
            load_snapshot_config(&HashMap::new()),
            SnapshotConfig {
                mode: SnapshotReaderMode::Auto,
                timeout: Duration::from_millis(4_000),
                cache_limit: 32,
            }
        );
        let config = load_snapshot_config(&HashMap::from([
            ("KIMI_SNAPSHOT_READER".into(), " LEGACY ".into()),
            ("KIMI_SNAPSHOT_TIMEOUT_MS".into(), "2500ms".into()),
            ("KIMI_SNAPSHOT_CACHE_LIMIT".into(), "0".into()),
        ]));
        assert_eq!(config.mode, SnapshotReaderMode::Legacy);
        assert_eq!(config.timeout, Duration::from_millis(2_500));
        assert_eq!(config.cache_limit, 32);
    }

    #[test]
    fn rejects_non_numeric_and_subminimum_values() {
        assert_eq!(
            load_snapshot_config(&HashMap::from([(
                "KIMI_SNAPSHOT_TIMEOUT_MS".into(),
                "abc".into()
            )]))
            .timeout,
            Duration::from_millis(4_000)
        );
        assert_eq!(
            load_snapshot_config(&HashMap::from([(
                "KIMI_SNAPSHOT_TIMEOUT_MS".into(),
                "50".into()
            )]))
            .timeout,
            Duration::from_millis(4_000)
        );
    }
}
