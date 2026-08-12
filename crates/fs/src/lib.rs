//! Shared filesystem operations with consistent cross-platform semantics.

use std::{
    io::{self, Write},
    path::Path,
};

#[cfg(any(not(windows), test))]
use std::path::PathBuf;

/// Atomically replaces `path` after fully writing and syncing its new contents.
///
/// The blocking filesystem operations are isolated from Tokio worker threads. The
/// parent directory must already exist. On Unix, `mode` is enforced on both new
/// and replacement files; on other platforms it is ignored.
pub async fn atomic_write(
    path: impl AsRef<Path>,
    contents: impl AsRef<[u8]>,
    mode: Option<u32>,
) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    let contents = contents.as_ref().to_owned();
    tokio::task::spawn_blocking(move || atomic_write_sync(&path, &contents, mode))
        .await
        .map_err(|error| io::Error::other(format!("atomic write task failed: {error}")))?
}

fn atomic_write_sync(path: &Path, contents: &[u8], mode: Option<u32>) -> io::Result<()> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut options = atomic_write_file::OpenOptions::new();
    #[cfg(unix)]
    if let Some(mode) = mode {
        use atomic_write_file::unix::OpenOptionsExt as AtomicOpenOptionsExt;
        use std::os::unix::fs::OpenOptionsExt as StdOpenOptionsExt;

        AtomicOpenOptionsExt::preserve_mode(&mut options, false);
        StdOpenOptionsExt::mode(&mut options, mode);
    }
    #[cfg(not(unix))]
    let _ = mode;

    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.commit()
}

/// Atomically writes a file and then syncs its parent directory where supported.
pub async fn atomic_write_durable(
    path: impl AsRef<Path>,
    contents: impl AsRef<[u8]>,
    mode: Option<u32>,
) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    let contents = contents.as_ref().to_owned();
    tokio::task::spawn_blocking(move || {
        atomic_write_sync(&path, &contents, mode)?;
        sync_parent_directory(&path)
    })
    .await
    .map_err(|error| io::Error::other(format!("durable atomic write task failed: {error}")))?
}

fn sync_parent_directory(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        std::fs::File::open(parent_directory(path))?.sync_all()
    }
}

#[cfg(not(windows))]
fn parent_directory(path: &Path) -> PathBuf {
    path.parent().unwrap_or_else(|| Path::new(".")).to_owned()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("kimi-code-fs-{}-{id}", std::process::id()))
    }

    #[tokio::test]
    async fn concurrent_writers_use_independent_temporary_files() {
        let directory = temp_dir();
        let _ = tokio::fs::remove_dir_all(&directory).await;
        tokio::fs::create_dir(&directory).await.unwrap();
        let path = directory.join("state.json");
        atomic_write_durable(&path, r#"{"writer":"initial"}"#, None)
            .await
            .unwrap();
        let payloads = (0..16)
            .map(|index| format!(r#"{{"writer":{index}}}"#))
            .collect::<Vec<_>>();

        let mut writers = tokio::task::JoinSet::new();
        for payload in &payloads {
            let path = path.clone();
            let payload = payload.clone();
            writers.spawn(async move { atomic_write(path, payload, Some(0o600)).await });
        }
        while let Some(result) = writers.join_next().await {
            result.unwrap().unwrap();
        }
        let final_contents = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(payloads.contains(&final_contents));
        assert_eq!(
            tokio::fs::read_dir(&directory)
                .await
                .unwrap()
                .entries()
                .await,
            vec!["state.json"]
        );
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn requested_mode_is_enforced_when_replacing_a_file() {
        use std::os::unix::fs::PermissionsExt;

        let directory = temp_dir();
        tokio::fs::create_dir(&directory).await.unwrap();
        let path = directory.join("private.json");
        tokio::fs::write(&path, "old").await.unwrap();
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .await
            .unwrap();

        atomic_write(&path, "new", Some(0o600)).await.unwrap();

        assert_eq!(
            tokio::fs::metadata(&path)
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    trait ReadDirEntries {
        async fn entries(self) -> Vec<String>;
    }

    impl ReadDirEntries for tokio::fs::ReadDir {
        async fn entries(mut self) -> Vec<String> {
            let mut names = Vec::new();
            while let Some(entry) = self.next_entry().await.unwrap() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
            names.sort();
            names
        }
    }
}
