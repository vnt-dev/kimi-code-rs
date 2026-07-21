use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use serde_json::Value;

const TERM_GRACE_MS: u64 = 3_000;
const KILL_GRACE_MS: u64 = 2_000;
const POLL_INTERVAL_MS: u64 = 100;
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

pub const LEGACY_SERVER_MAX_VERSION: &str = "0.28.0";
pub const DEPRECATED_KILL_NOTICE: &str = "`kimi server kill` is deprecated: it only stops servers started by a version before 0.28.0. Servers started by `kimi web` run in the foreground — stop them with Ctrl+C.\n";

#[derive(Debug, Clone, PartialEq)]
pub struct LegacyServerLock {
    pub pid: i64,
    pub host: Option<String>,
    pub port: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSignal {
    Term,
    Kill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyKillOutcome {
    Stopped,
    Killed,
}

impl LegacyKillOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Killed => "killed",
        }
    }
}

#[derive(Debug)]
pub struct LegacyKillError(Box<dyn Error + Send + Sync>);

impl LegacyKillError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }

    fn message(message: impl Into<String>) -> Self {
        Self(Box::new(LegacyKillMessage(message.into())))
    }
}

impl fmt::Display for LegacyKillError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for LegacyKillError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[derive(Debug)]
struct LegacyKillMessage(String);

impl fmt::Display for LegacyKillMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for LegacyKillMessage {}

#[async_trait]
pub trait LegacyKillRuntime: Send + Sync {
    async fn read_lock(&self) -> Result<Option<LegacyServerLock>, LegacyKillError>;

    async fn remove_lock(&self) -> Result<(), LegacyKillError>;

    async fn request_shutdown(
        &self,
        origin: &str,
        token: Option<&str>,
    ) -> Result<(), LegacyKillError>;

    fn resolve_token(&self) -> Option<String>;

    fn signal_pid(&self, pid: i64, signal: ProcessSignal) -> bool;

    fn pid_alive(&self, pid: i64) -> bool;

    async fn sleep(&self, milliseconds: u64);

    fn now_millis(&self) -> u64;

    fn write_stdout(&self, text: &str);

    fn write_stderr(&self, text: &str);
}

// Original:
//   apps/kimi-code/src/cli/sub/web/legacy-kill.ts
//   handleLegacyKillCommand()
pub async fn handle_legacy_kill(runtime: &dyn LegacyKillRuntime) -> Result<(), LegacyKillError> {
    runtime.write_stderr(DEPRECATED_KILL_NOTICE);
    let Some(lock) = runtime.read_lock().await? else {
        runtime.write_stdout("No running legacy Kimi server.\n");
        return Ok(());
    };
    if !runtime.pid_alive(lock.pid) {
        let _ = runtime.remove_lock().await;
        runtime.write_stdout("No running legacy Kimi server.\n");
        return Ok(());
    }

    let outcome = kill_legacy_server(&lock, runtime).await?;
    let _ = runtime.remove_lock().await;
    runtime.write_stdout(&format!(
        "Legacy Kimi server (pid {}) {}.\n",
        lock.pid,
        outcome.as_str()
    ));
    Ok(())
}

// Original:
//   apps/kimi-code/src/cli/sub/web/legacy-kill.ts
//   killLegacyServer()
pub async fn kill_legacy_server(
    lock: &LegacyServerLock,
    runtime: &dyn LegacyKillRuntime,
) -> Result<LegacyKillOutcome, LegacyKillError> {
    if let Some(port) = lock.port {
        let host = lock.host.as_deref().unwrap_or("127.0.0.1");
        let origin = format!("http://{host}:{port}");
        let token = runtime.resolve_token();
        let _ = runtime.request_shutdown(&origin, token.as_deref()).await;
    }

    runtime.signal_pid(lock.pid, ProcessSignal::Term);
    if wait_for_exit(lock.pid, TERM_GRACE_MS, runtime).await {
        return Ok(LegacyKillOutcome::Stopped);
    }

    runtime.signal_pid(lock.pid, ProcessSignal::Kill);
    if wait_for_exit(lock.pid, KILL_GRACE_MS, runtime).await {
        return Ok(LegacyKillOutcome::Killed);
    }

    Err(LegacyKillError::message(format!(
        "Failed to stop legacy Kimi server (pid {}); insufficient permissions?",
        lock.pid
    )))
}

async fn wait_for_exit(pid: i64, timeout_ms: u64, runtime: &dyn LegacyKillRuntime) -> bool {
    let deadline = runtime.now_millis().saturating_add(timeout_ms);
    loop {
        if !runtime.pid_alive(pid) {
            return true;
        }
        runtime.sleep(POLL_INTERVAL_MS).await;
        if runtime.now_millis() >= deadline {
            break;
        }
    }
    !runtime.pid_alive(pid)
}

pub fn legacy_lock_path(home_dir: &Path) -> PathBuf {
    home_dir.join("server").join("lock")
}

// Original:
//   apps/kimi-code/src/cli/sub/web/legacy-kill.ts
//   readLegacyLock()
pub async fn read_legacy_lock(lock_path: &Path) -> Option<LegacyServerLock> {
    let raw = tokio::fs::read_to_string(lock_path).await.ok()?;
    let parsed = serde_json::from_str::<Value>(&raw).ok()?;
    let object = parsed.as_object()?;
    let pid = object.get("pid")?.as_i64()?;
    if !(1..=MAX_SAFE_INTEGER).contains(&pid) {
        return None;
    }
    Some(LegacyServerLock {
        pid,
        host: object
            .get("host")
            .and_then(Value::as_str)
            .map(str::to_owned),
        port: object.get("port").and_then(Value::as_f64),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::Mutex,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    struct RuntimeMock {
        lock: Option<LegacyServerLock>,
        alive_until: u64,
        clock: Mutex<u64>,
        shutdown_calls: Mutex<Vec<(String, Option<String>)>>,
        remove_calls: Mutex<usize>,
        signals: Mutex<Vec<(i64, ProcessSignal)>>,
        token: Option<String>,
        stdout: Mutex<String>,
        stderr: Mutex<String>,
    }

    impl RuntimeMock {
        fn new(lock: Option<LegacyServerLock>, alive_until: u64) -> Self {
            Self {
                lock,
                alive_until,
                clock: Mutex::new(0),
                shutdown_calls: Mutex::new(Vec::new()),
                remove_calls: Mutex::new(0),
                signals: Mutex::new(Vec::new()),
                token: None,
                stdout: Mutex::new(String::new()),
                stderr: Mutex::new(String::new()),
            }
        }
    }

    #[async_trait]
    impl LegacyKillRuntime for RuntimeMock {
        async fn read_lock(&self) -> Result<Option<LegacyServerLock>, LegacyKillError> {
            Ok(self.lock.clone())
        }

        async fn remove_lock(&self) -> Result<(), LegacyKillError> {
            *self.remove_calls.lock().expect("remove calls") += 1;
            Ok(())
        }

        async fn request_shutdown(
            &self,
            origin: &str,
            token: Option<&str>,
        ) -> Result<(), LegacyKillError> {
            self.shutdown_calls
                .lock()
                .expect("shutdown calls")
                .push((origin.to_owned(), token.map(str::to_owned)));
            Ok(())
        }

        fn resolve_token(&self) -> Option<String> {
            self.token.clone()
        }

        fn signal_pid(&self, pid: i64, signal: ProcessSignal) -> bool {
            self.signals.lock().expect("signals").push((pid, signal));
            true
        }

        fn pid_alive(&self, _: i64) -> bool {
            *self.clock.lock().expect("clock") < self.alive_until
        }

        async fn sleep(&self, milliseconds: u64) {
            *self.clock.lock().expect("clock") += milliseconds;
        }

        fn now_millis(&self) -> u64 {
            *self.clock.lock().expect("clock")
        }

        fn write_stdout(&self, text: &str) {
            self.stdout.lock().expect("stdout").push_str(text);
        }

        fn write_stderr(&self, text: &str) {
            self.stderr.lock().expect("stderr").push_str(text);
        }
    }

    fn lock(pid: i64) -> LegacyServerLock {
        LegacyServerLock {
            pid,
            host: Some("127.0.0.1".to_owned()),
            port: Some(58_627.0),
        }
    }

    #[tokio::test]
    async fn always_prints_notice_and_handles_missing_lock_without_signals() {
        let runtime = RuntimeMock::new(None, 0);
        handle_legacy_kill(&runtime).await.expect("no server");
        assert_eq!(
            runtime.stderr.lock().expect("stderr").as_str(),
            DEPRECATED_KILL_NOTICE
        );
        assert_eq!(
            runtime.stdout.lock().expect("stdout").as_str(),
            "No running legacy Kimi server.\n"
        );
        assert!(runtime.signals.lock().expect("signals").is_empty());
    }

    #[tokio::test]
    async fn stale_lock_is_swept_without_api_or_signal() {
        let runtime = RuntimeMock::new(Some(lock(1_234)), 0);
        handle_legacy_kill(&runtime).await.expect("stale");
        assert_eq!(*runtime.remove_calls.lock().expect("remove"), 1);
        assert!(runtime.shutdown_calls.lock().expect("shutdown").is_empty());
        assert!(runtime.signals.lock().expect("signals").is_empty());
    }

    #[tokio::test]
    async fn requests_api_then_stops_after_sigterm_and_removes_lock() {
        let mut runtime = RuntimeMock::new(Some(lock(1_234)), 50);
        runtime.token = Some("tok-123".to_owned());
        handle_legacy_kill(&runtime).await.expect("stopped");
        assert_eq!(
            runtime.shutdown_calls.lock().expect("shutdown").as_slice(),
            [(
                "http://127.0.0.1:58627".to_owned(),
                Some("tok-123".to_owned())
            )]
        );
        assert_eq!(
            runtime.signals.lock().expect("signals").as_slice(),
            [(1_234, ProcessSignal::Term)]
        );
        assert!(
            runtime
                .stdout
                .lock()
                .expect("stdout")
                .contains("pid 1234) stopped.")
        );
        assert_eq!(*runtime.remove_calls.lock().expect("remove"), 1);
    }

    #[tokio::test]
    async fn escalates_to_sigkill_when_term_grace_expires() {
        let runtime = RuntimeMock::new(Some(lock(5_678)), 3_100);
        handle_legacy_kill(&runtime).await.expect("killed");
        assert_eq!(
            runtime.signals.lock().expect("signals").as_slice(),
            [(5_678, ProcessSignal::Term), (5_678, ProcessSignal::Kill)]
        );
        assert!(
            runtime
                .stdout
                .lock()
                .expect("stdout")
                .contains("pid 5678) killed.")
        );
    }

    #[tokio::test]
    async fn surviving_sigkill_returns_permissions_error_without_removing_lock() {
        let runtime = RuntimeMock::new(Some(lock(9_999)), u64::MAX);
        let error = handle_legacy_kill(&runtime).await.expect_err("survives");
        assert!(error.to_string().contains("insufficient permissions"));
        assert_eq!(*runtime.remove_calls.lock().expect("remove"), 0);
    }

    #[tokio::test]
    async fn missing_port_skips_api_path() {
        let runtime = RuntimeMock::new(
            Some(LegacyServerLock {
                pid: 1_234,
                host: None,
                port: None,
            }),
            50,
        );
        handle_legacy_kill(&runtime).await.expect("stopped");
        assert!(runtime.shutdown_calls.lock().expect("shutdown").is_empty());
    }

    #[tokio::test]
    async fn reads_old_lock_shape_and_rejects_unsafe_pids() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("kimi-legacy-lock-{unique}"));
        fs::create_dir_all(&directory).expect("directory");
        let path = directory.join("lock");
        fs::write(
            &path,
            r#"{"pid":1234,"started_at":"2026-01-01","port":58627}"#,
        )
        .expect("lock");
        assert_eq!(
            read_legacy_lock(&path).await,
            Some(LegacyServerLock {
                pid: 1_234,
                host: None,
                port: Some(58_627.0),
            })
        );
        for invalid in ["0", "-1", "1.5", "\"1234\""] {
            fs::write(&path, format!("{{\"pid\":{invalid}}}")).expect("invalid lock");
            assert_eq!(read_legacy_lock(&path).await, None);
        }
        fs::write(&path, "not json").expect("corrupt lock");
        assert_eq!(read_legacy_lock(&path).await, None);
        assert_eq!(read_legacy_lock(&directory.join("missing")).await, None);
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn builds_legacy_lock_path() {
        assert_eq!(
            legacy_lock_path(Path::new("/tmp/kimi")),
            Path::new("/tmp/kimi").join("server").join("lock")
        );
    }
}
