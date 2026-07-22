use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::rename_replace::{RenameReplaceOptions, rename_replace};

const TAKEOVER_SETTLE_BASE: Duration = Duration::from_millis(60);
const TAKEOVER_SETTLE_MAX: Duration = Duration::from_secs(2);
static SIDECAR_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum LockError {
    #[error("database is locked by another process: {0}")]
    Locked(PathBuf),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Serialize, Deserialize)]
struct LockOwner {
    pid: u32,
    ts: u128,
}

#[derive(Debug)]
struct Inspection {
    alive: bool,
    mine: bool,
}

pub struct LockFile {
    pub path: PathBuf,
    held: AtomicBool,
}

impl LockFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            held: AtomicBool::new(false),
        }
    }

    pub fn is_held(&self) -> bool {
        self.held.load(Ordering::Acquire)
    }

    /// Attempt exactly once. Observing any live owner or competing takeover
    /// returns false; waiting/retry policy belongs to the caller.
    // Original: packages/minidb/src/lockfile.ts, LockFile.acquire().
    pub async fn acquire(&self) -> Result<bool, LockError> {
        let watch = self.sidecar("watch");
        tokio::fs::write(&watch, owner_bytes()?).await?;
        let result = self.acquire_inner().await;
        let _ = tokio::fs::remove_file(&watch).await;
        result
    }

    async fn acquire_inner(&self) -> Result<bool, LockError> {
        self.reap_dead_watches().await?;
        if self.try_create().await? {
            return Ok(true);
        }
        let Some(seen) = self.inspect().await? else {
            return Ok(false);
        };
        if seen.alive {
            return Ok(false);
        }

        let bid = self.sidecar("bid");
        let attempt_started = Instant::now();
        tokio::fs::write(&bid, owner_bytes()?).await?;
        // On POSIX rename either succeeds or returns immediately; Windows uses
        // every iteration to ride out transient destination-handle EPERM.
        #[allow(clippy::never_loop)]
        let takeover = async {
            for _attempt in 0..=50_u64 {
                let Some(gate) = self.inspect().await? else {
                    return Ok(false);
                };
                if gate.alive || gate.mine {
                    return Ok(false);
                }
                match tokio::fs::rename(&bid, &self.path).await {
                    Ok(()) => return Ok(true),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
                    #[cfg(windows)]
                    Err(error)
                        if error.kind() == io::ErrorKind::PermissionDenied && _attempt < 50 =>
                    {
                        tokio::time::sleep(Duration::from_millis(20 + (_attempt * 17) % 30)).await;
                    }
                    Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                        return Ok(false);
                    }
                    Err(error) => return Err(LockError::Io(error)),
                }
            }
            Ok(false)
        }
        .await;
        let won_replace = match takeover {
            Ok(won) => won,
            Err(error) => {
                let _ = tokio::fs::remove_file(&bid).await;
                return Err(error);
            }
        };
        let _ = tokio::fs::remove_file(&bid).await;
        if !won_replace {
            return Ok(false);
        }

        let elapsed = attempt_started.elapsed();
        let mut settle = elapsed
            .saturating_mul(4)
            .max(TAKEOVER_SETTLE_BASE)
            .min(TAKEOVER_SETTLE_MAX);
        loop {
            tokio::time::sleep(settle).await;
            if !self.inspect().await?.is_some_and(|owner| owner.mine) {
                return Ok(false);
            }
            if !self.has_live_foreign_watch().await? {
                break;
            }
            settle = settle.saturating_mul(2).min(TAKEOVER_SETTLE_MAX);
        }
        self.held.store(true, Ordering::Release);
        Ok(true)
    }

    async fn try_create(&self) -> Result<bool, LockError> {
        let temporary = self.sidecar("tmp");
        tokio::fs::write(&temporary, owner_bytes()?).await?;
        let result = match tokio::fs::hard_link(&temporary, &self.path).await {
            Ok(()) => {
                self.held.store(true, Ordering::Release);
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(error.into()),
        };
        let _ = tokio::fs::remove_file(temporary).await;
        result
    }

    async fn inspect(&self) -> Result<Option<Inspection>, LockError> {
        let bytes = match tokio::fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let pid = serde_json::from_slice::<LockOwner>(&bytes)
            .ok()
            .map(|owner| owner.pid);
        Ok(Some(Inspection {
            alive: pid.is_some_and(pid_alive),
            mine: pid == Some(std::process::id()),
        }))
    }

    fn inspect_sync(&self) -> io::Result<Option<Inspection>> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let pid = serde_json::from_slice::<LockOwner>(&bytes)
            .ok()
            .map(|owner| owner.pid);
        Ok(Some(Inspection {
            alive: pid.is_some_and(pid_alive),
            mine: pid == Some(std::process::id()),
        }))
    }

    async fn reap_dead_watches(&self) -> Result<(), LockError> {
        let (directory, prefix) = self.watch_location();
        let mut entries = match tokio::fs::read_dir(directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(pid) = watch_pid(&name, &prefix) else {
                continue;
            };
            if pid != std::process::id() && !pid_alive(pid) {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
        Ok(())
    }

    async fn has_live_foreign_watch(&self) -> Result<bool, LockError> {
        let (directory, prefix) = self.watch_location();
        let mut entries = match tokio::fs::read_dir(directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(pid) = watch_pid(&name, &prefix) else {
                continue;
            };
            if pid == std::process::id() {
                continue;
            }
            if pid_alive(pid) {
                return Ok(true);
            }
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
        Ok(false)
    }

    // Original: LockFile.renew(). Atomic replacement prevents truncated live-owner records.
    pub async fn renew(&self) -> Result<(), LockError> {
        if !self.is_held() {
            return Ok(());
        }
        let temporary = self.sidecar("tmp");
        tokio::fs::write(&temporary, owner_bytes()?).await?;
        if let Err(error) = rename_replace(
            &temporary,
            &self.path,
            RenameReplaceOptions {
                retries: 20,
                ..Default::default()
            },
        )
        .await
        {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(error.into());
        }
        Ok(())
    }

    pub async fn release(&self) -> Result<(), LockError> {
        if !self.is_held() {
            return Ok(());
        }
        if self.inspect().await?.is_some_and(|owner| owner.mine) {
            match tokio::fs::remove_file(&self.path).await {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        self.held.store(false, Ordering::Release);
        Ok(())
    }

    pub fn release_sync(&self) {
        if !self.is_held() {
            return;
        }
        if self
            .inspect_sync()
            .ok()
            .flatten()
            .is_some_and(|owner| owner.mine)
        {
            let _ = fs::remove_file(&self.path);
        }
        self.held.store(false, Ordering::Release);
    }

    fn sidecar(&self, kind: &str) -> PathBuf {
        let sequence = SIDECAR_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
        PathBuf::from(format!(
            "{}.{}-{}-{sequence}",
            self.path.display(),
            kind,
            std::process::id()
        ))
    }

    fn watch_location(&self) -> (&Path, String) {
        let directory = self.path.parent().unwrap_or_else(|| Path::new("."));
        let name = self.path.file_name().unwrap_or_default().to_string_lossy();
        (directory, format!("{name}.watch-"))
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        self.release_sync();
    }
}

fn watch_pid(name: &str, prefix: &str) -> Option<u32> {
    name.strip_prefix(prefix)?.split('-').next()?.parse().ok()
}

fn owner_bytes() -> Result<Vec<u8>, serde_json::Error> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    serde_json::to_vec(&LockOwner {
        pid: std::process::id(),
        ts,
    })
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: kill(pid, 0) sends no signal; it only performs the OS liveness/permission check.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn pid_alive(pid: u32) -> bool {
    pid == std::process::id()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn excludes_live_owner_and_releases_only_owned_lock() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("db.lock");
        let first = LockFile::new(&path);
        let second = LockFile::new(&path);
        assert!(first.acquire().await.unwrap());
        assert!(!second.acquire().await.unwrap());
        first.renew().await.unwrap();
        first.release().await.unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn takes_over_dead_owner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("db.lock");
        tokio::fs::write(&path, br#"{"pid":4294967295,"ts":0}"#)
            .await
            .unwrap();
        let lock = LockFile::new(&path);
        assert!(lock.acquire().await.unwrap());
        lock.release().await.unwrap();
    }
}
