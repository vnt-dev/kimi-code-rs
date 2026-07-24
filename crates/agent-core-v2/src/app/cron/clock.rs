//! Wall-clock and monotonic clock sources for cron scheduling.
//!
//! Original: `packages/agent-core-v2/src/app/cron/clock.ts`.
//!
//! Rust keeps wall time and monotonic elapsed time separate for the same
//! reason as the source: cron matching must use user-visible time, while
//! polling and lock heartbeats must not be affected by wall-clock changes.

use std::{
    fs::File,
    io::Read,
    path::PathBuf,
    sync::{Arc, LazyLock},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const MAX_CLOCK_FILE_BYTES: usize = 64;

static MONOTONIC_EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

pub trait ClockSources: Send + Sync {
    // Original: ClockSources.wallNow(). Milliseconds are a floating-point
    // value because the source's JavaScript `Number` accepts finite decimals.
    fn wall_now_ms(&self) -> f64;

    // Original: ClockSources.monoNowMs().
    fn mono_now_ms(&self) -> f64;
}

#[derive(Debug, Default)]
pub struct SystemClocks;

impl ClockSources for SystemClocks {
    fn wall_now_ms(&self) -> f64 {
        system_wall_now_ms()
    }

    fn mono_now_ms(&self) -> f64 {
        system_mono_now_ms()
    }
}

// Original: SYSTEM_CLOCKS. A zero-sized static is the Rust equivalent of the
// source's immutable module-level clock object.
pub static SYSTEM_CLOCKS: SystemClocks = SystemClocks;

#[derive(Debug)]
struct FileWallClock {
    path: PathBuf,
}

impl ClockSources for FileWallClock {
    fn wall_now_ms(&self) -> f64 {
        read_file_wall(&self.path)
    }

    fn mono_now_ms(&self) -> f64 {
        system_mono_now_ms()
    }
}

// Original: resolveClockSources(). The source receives the environment value
// from cron configuration; it deliberately does not read environment state at
// this method boundary.
pub fn resolve_clock_sources(spec: Option<&str>, debug: bool) -> Arc<dyn ClockSources> {
    let Some(spec) = spec.filter(|spec| !spec.is_empty() && *spec != "system") else {
        return Arc::new(SystemClocks);
    };
    if let Some(path) = spec.strip_prefix("file:") {
        if path.is_empty() {
            debug_invalid_spec(spec, "empty file path", debug);
            return Arc::new(SystemClocks);
        }
        return Arc::new(FileWallClock {
            path: PathBuf::from(path),
        });
    }
    debug_invalid_spec(spec, "unrecognised scheme", debug);
    Arc::new(SystemClocks)
}

fn system_wall_now_ms() -> f64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs_f64() * 1_000.0,
        Err(error) => -(error.duration().as_secs_f64() * 1_000.0),
    }
}

fn system_mono_now_ms() -> f64 {
    MONOTONIC_EPOCH.elapsed().as_secs_f64() * 1_000.0
}

// Original: readFileWall(). It intentionally uses a bounded synchronous read:
// this is a tiny test/benchmark clock file, not regular application I/O.
fn read_file_wall(path: &std::path::Path) -> f64 {
    let mut bytes = [0_u8; MAX_CLOCK_FILE_BYTES];
    let Ok(mut file) = File::open(path) else {
        return system_wall_now_ms();
    };
    let Ok(bytes_read) = file.read(&mut bytes) else {
        return system_wall_now_ms();
    };
    let raw = String::from_utf8_lossy(&bytes[..bytes_read]);
    let first_line = raw.split('\n').next().unwrap_or_default().trim();
    if first_line.is_empty() {
        return system_wall_now_ms();
    }
    parse_js_number(first_line).unwrap_or_else(system_wall_now_ms)
}

// Original: `Number(firstLine)` followed by `Number.isFinite`. The decimal
// parser covers ordinary JSON-like values; the prefixed forms preserve the
// JavaScript Number constructor's accepted hexadecimal, binary, and octal
// integer spellings as well.
fn parse_js_number(value: &str) -> Option<f64> {
    let parsed = if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u128::from_str_radix(hex, 16)
            .ok()
            .map(|number| number as f64)
    } else if let Some(binary) = value
        .strip_prefix("0b")
        .or_else(|| value.strip_prefix("0B"))
    {
        u128::from_str_radix(binary, 2)
            .ok()
            .map(|number| number as f64)
    } else if let Some(octal) = value
        .strip_prefix("0o")
        .or_else(|| value.strip_prefix("0O"))
    {
        u128::from_str_radix(octal, 8)
            .ok()
            .map(|number| number as f64)
    } else {
        value.parse::<f64>().ok()
    };
    parsed.filter(|number| number.is_finite())
}

fn debug_invalid_spec(spec: &str, reason: &str, debug: bool) {
    if debug {
        let quoted = serde_json::to_string(spec).unwrap_or_else(|_| "<invalid>".into());
        eprintln!(
            "[cron/clock] invalid KIMI_CRON_CLOCK spec {quoted}: {reason} — falling back to system clock"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

    fn clock_file(contents: &str) -> PathBuf {
        let suffix = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "kimi-code-cron-clock-{}-{suffix}",
            std::process::id()
        ));
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn system_clock_has_wall_and_non_decreasing_monotonic_milliseconds() {
        let first = SYSTEM_CLOCKS.mono_now_ms();
        let second = SYSTEM_CLOCKS.mono_now_ms();
        assert!(SYSTEM_CLOCKS.wall_now_ms().is_finite());
        assert!(second >= first);
    }

    #[test]
    fn file_clock_reads_the_trimmed_first_line_and_keeps_system_monotonic_time() {
        let path = clock_file(" 0x10 \n999");
        let clocks = resolve_clock_sources(Some(&format!("file:{}", path.display())), false);
        assert_eq!(clocks.wall_now_ms(), 16.0);
        assert!(clocks.mono_now_ms().is_finite());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn invalid_specs_and_invalid_file_values_fall_back_to_system_wall_time() {
        let system = resolve_clock_sources(Some("system"), false);
        let invalid = resolve_clock_sources(Some("unknown:value"), false);
        assert!(system.wall_now_ms().is_finite());
        assert!(invalid.wall_now_ms().is_finite());

        let path = clock_file("Infinity\n");
        let clocks = resolve_clock_sources(Some(&format!("file:{}", path.display())), false);
        assert!(clocks.wall_now_ms().is_finite());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn number_parser_preserves_finite_javascript_number_forms() {
        assert_eq!(parse_js_number("1.25e3"), Some(1_250.0));
        assert_eq!(parse_js_number("0b101"), Some(5.0));
        assert_eq!(parse_js_number("0o10"), Some(8.0));
        assert_eq!(parse_js_number("+0x10"), None);
        assert_eq!(parse_js_number("-0x10"), None);
        assert_eq!(parse_js_number("NaN"), None);
        assert_eq!(parse_js_number("Infinity"), None);
    }
}
