use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const AUTH_RATE_LIMIT_CODE: i64 = 42_901;
pub const AUTH_RATE_LIMIT_MSG: &str = "Too many failed auth attempts";

#[derive(Clone)]
pub struct AuthFailureLimiterOptions {
    pub max_failures: u32,
    pub window: Duration,
    pub ban: Duration,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl Default for AuthFailureLimiterOptions {
    fn default() -> Self {
        Self {
            max_failures: 10,
            window: Duration::from_secs(60),
            ban: Duration::from_secs(60),
            now: Arc::new(now_millis),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    count: u32,
    window_start: u64,
    banned_until: u64,
}

pub struct AuthFailureLimiter {
    options: AuthFailureLimiterOptions,
    entries: Mutex<HashMap<String, Entry>>,
}

impl std::fmt::Debug for AuthFailureLimiter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthFailureLimiter")
            .field("max_failures", &self.options.max_failures)
            .field("window", &self.options.window)
            .field("ban", &self.options.ban)
            .finish_non_exhaustive()
    }
}

impl AuthFailureLimiter {
    pub fn new(options: AuthFailureLimiterOptions) -> Self {
        Self {
            options,
            entries: Mutex::new(HashMap::new()),
        }
    }

    // Original: rateLimit.ts, recordFailure().
    pub fn record_failure(&self, ip: &str) -> bool {
        let now = (self.options.now)();
        let window_ms = self.options.window.as_millis() as u64;
        let ban_ms = self.options.ban.as_millis() as u64;
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        entries.retain(|_, entry| {
            entry.banned_until > now || now.saturating_sub(entry.window_start) <= window_ms
        });
        let entry = entries.entry(ip.to_owned()).or_insert(Entry {
            count: 0,
            window_start: now,
            banned_until: 0,
        });
        if now.saturating_sub(entry.window_start) > window_ms {
            *entry = Entry {
                count: 0,
                window_start: now,
                banned_until: 0,
            };
        }
        entry.count = entry.count.saturating_add(1);
        let was_banned = entry.banned_until > now;
        if entry.count >= self.options.max_failures {
            entry.banned_until = now.saturating_add(ban_ms);
        }
        !was_banned && entry.banned_until > now
    }

    pub fn is_banned(&self, ip: &str) -> bool {
        let now = (self.options.now)();
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(ip)
            .is_some_and(|entry| entry.banned_until > now)
    }

    pub fn dispose(&self) {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn limiter(now: Arc<AtomicU64>) -> AuthFailureLimiter {
        AuthFailureLimiter::new(AuthFailureLimiterOptions {
            max_failures: 2,
            window: Duration::from_millis(1_000),
            ban: Duration::from_millis(500),
            now: Arc::new(move || now.load(Ordering::SeqCst)),
        })
    }

    #[test]
    fn bans_at_threshold_and_expires() {
        let now = Arc::new(AtomicU64::new(0));
        let limiter = limiter(Arc::clone(&now));
        assert!(!limiter.record_failure("1.2.3.4"));
        assert!(limiter.record_failure("1.2.3.4"));
        assert!(limiter.is_banned("1.2.3.4"));
        assert!(!limiter.is_banned("8.8.8.8"));
        now.store(499, Ordering::SeqCst);
        assert!(limiter.is_banned("1.2.3.4"));
        now.store(500, Ordering::SeqCst);
        assert!(!limiter.is_banned("1.2.3.4"));
    }

    #[test]
    fn resets_count_after_strictly_more_than_window() {
        let now = Arc::new(AtomicU64::new(0));
        let limiter = limiter(Arc::clone(&now));
        limiter.record_failure("1.2.3.4");
        now.store(1_001, Ordering::SeqCst);
        limiter.record_failure("1.2.3.4");
        assert!(!limiter.is_banned("1.2.3.4"));
    }
}
