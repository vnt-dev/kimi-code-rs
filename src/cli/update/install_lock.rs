use std::{
    error::Error,
    fmt,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use chrono::{DateTime, SecondsFormat, Utc};

use crate::utils::paths::{HomeDirectoryUnavailable, get_update_install_lock_file};

const UPDATE_INSTALL_LOCK_STALE: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInstallLockRequest {
    pub version: String,
    pub now: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInstallLockHandle {
    pub file_path: PathBuf,
}

impl UpdateInstallLockHandle {
    pub async fn release(&self) -> io::Result<()> {
        match tokio::fs::remove_file(&self.file_path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[derive(Debug)]
pub enum UpdateInstallLockError {
    Path(HomeDirectoryUnavailable),
    Io(io::Error),
    Join(tokio::task::JoinError),
}

impl fmt::Display for UpdateInstallLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(error) => error.fmt(formatter),
            Self::Io(error) => error.fmt(formatter),
            Self::Join(error) => write!(formatter, "update lock task failed: {error}"),
        }
    }
}

impl Error for UpdateInstallLockError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Path(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Join(error) => Some(error),
        }
    }
}

// Original:
//   apps/kimi-code/src/cli/update/install-lock.ts
//   tryAcquireUpdateInstallLock()
pub async fn try_acquire_update_install_lock(
    request: &UpdateInstallLockRequest,
) -> Result<Option<UpdateInstallLockHandle>, UpdateInstallLockError> {
    let file_path = get_update_install_lock_file().map_err(UpdateInstallLockError::Path)?;
    try_acquire_update_install_lock_at(request, &file_path).await
}

pub async fn try_acquire_update_install_lock_at(
    request: &UpdateInstallLockRequest,
    file_path: &Path,
) -> Result<Option<UpdateInstallLockHandle>, UpdateInstallLockError> {
    if let Some(parent) = file_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(UpdateInstallLockError::Io)?;
    }
    let now = request
        .now
        .unwrap_or_else(|| DateTime::<Utc>::from(SystemTime::now()));
    match create_lock_file(file_path, &request.version, now).await {
        Ok(handle) => return Ok(Some(handle)),
        Err(UpdateInstallLockError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }

    if !is_stale_lock(file_path, now).await {
        return Ok(None);
    }
    match tokio::fs::remove_file(file_path).await {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(UpdateInstallLockError::Io(error)),
    }

    match create_lock_file(file_path, &request.version, now).await {
        Ok(handle) => Ok(Some(handle)),
        Err(UpdateInstallLockError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

async fn is_stale_lock(file_path: &Path, now: DateTime<Utc>) -> bool {
    let raw = match tokio::fs::read_to_string(file_path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return true,
        Err(_) => return false,
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return true;
    };
    let Some(started_at) = value.get("startedAt").and_then(serde_json::Value::as_str) else {
        return true;
    };
    let Ok(started_at) = DateTime::parse_from_rfc3339(started_at) else {
        return true;
    };
    now.signed_duration_since(started_at.to_utc())
        .to_std()
        .is_ok_and(|elapsed| elapsed > UPDATE_INSTALL_LOCK_STALE)
}

async fn create_lock_file(
    file_path: &Path,
    version: &str,
    now: DateTime<Utc>,
) -> Result<UpdateInstallLockHandle, UpdateInstallLockError> {
    let content = serde_json::to_string_pretty(&serde_json::json!({
        "version": version,
        "pid": std::process::id(),
        "startedAt": now.to_rfc3339_opts(SecondsFormat::Millis, true),
    }))
    .map_err(|error| {
        UpdateInstallLockError::Io(io::Error::new(io::ErrorKind::InvalidData, error))
    })? + "\n";
    let file_path = file_path.to_path_buf();
    let write_path = file_path.clone();
    tokio::task::spawn_blocking(move || create_new_private_file(&write_path, content.as_bytes()))
        .await
        .map_err(UpdateInstallLockError::Join)?
        .map_err(UpdateInstallLockError::Io)?;
    Ok(UpdateInstallLockHandle { file_path })
}

fn create_new_private_file(file_path: &Path, content: &[u8]) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(file_path)?;
    file.write_all(content)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use chrono::TimeZone;

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp_file() -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!("kimi-install-lock-{}-{id}", std::process::id()))
            .join("updates")
            .join("install.lock")
    }

    fn request(now: DateTime<Utc>) -> UpdateInstallLockRequest {
        UpdateInstallLockRequest {
            version: "0.5.0".to_owned(),
            now: Some(now),
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 21, 8, 0, 0)
            .single()
            .expect("valid time")
    }

    async fn cleanup(file: &Path) {
        if let Some(root) = file.parent().and_then(Path::parent) {
            let _ = tokio::fs::remove_dir_all(root).await;
        }
    }

    #[tokio::test]
    async fn allows_only_one_holder_until_release() {
        let file = temp_file();
        let first = try_acquire_update_install_lock_at(&request(now()), &file)
            .await
            .expect("first acquire")
            .expect("first holder");
        assert_eq!(
            try_acquire_update_install_lock_at(&request(now()), &file)
                .await
                .expect("second acquire"),
            None
        );
        first.release().await.expect("release");
        let third = try_acquire_update_install_lock_at(&request(now()), &file)
            .await
            .expect("third acquire")
            .expect("third holder");
        third.release().await.expect("release third");
        cleanup(&file).await;
    }

    #[tokio::test]
    async fn recovers_from_corrupt_and_expired_lock_files() {
        let file = temp_file();
        tokio::fs::create_dir_all(file.parent().expect("parent"))
            .await
            .expect("parent");
        tokio::fs::write(&file, "{").await.expect("corrupt lock");
        let corrupt = try_acquire_update_install_lock_at(&request(now()), &file)
            .await
            .expect("recover corrupt")
            .expect("corrupt replacement");
        corrupt.release().await.expect("release corrupt");

        let old = now() - chrono::TimeDelta::minutes(31);
        tokio::fs::write(
            &file,
            serde_json::to_vec(&serde_json::json!({
                "version": "0.4.0", "pid": 1, "startedAt": old.to_rfc3339()
            }))
            .expect("old lock json"),
        )
        .await
        .expect("old lock");
        let expired = try_acquire_update_install_lock_at(&request(now()), &file)
            .await
            .expect("recover expired")
            .expect("expired replacement");
        expired.release().await.expect("release expired");
        cleanup(&file).await;
    }

    #[tokio::test]
    async fn exactly_thirty_minutes_old_is_not_stale() {
        let file = temp_file();
        tokio::fs::create_dir_all(file.parent().expect("parent"))
            .await
            .expect("parent");
        let started_at = now() - chrono::TimeDelta::minutes(30);
        tokio::fs::write(
            &file,
            serde_json::to_vec(&serde_json::json!({ "startedAt": started_at.to_rfc3339() }))
                .expect("lock json"),
        )
        .await
        .expect("lock");
        assert_eq!(
            try_acquire_update_install_lock_at(&request(now()), &file)
                .await
                .expect("acquire"),
            None
        );
        cleanup(&file).await;
    }
}
