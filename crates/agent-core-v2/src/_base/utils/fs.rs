use std::path::Path;

use futures_util::future::BoxFuture;
use tokio::{
    fs::{self, File, OpenOptions},
    io::AsyncWriteExt,
};
use uuid::Uuid;

pub async fn sync_dir(path: impl AsRef<Path>) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        File::open(path).await?.sync_all().await
    }
}

pub fn sync_dir_sync(path: impl AsRef<Path>) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        std::fs::File::open(path)?.sync_all()
    }
}

// Original: packages/agent-core-v2/src/_base/utils/fs.ts, writeFileAtomicDurable().
pub async fn write_file_atomic_durable(
    file_path: impl AsRef<Path>,
    content: impl AsRef<[u8]>,
) -> std::io::Result<()> {
    let file_path = file_path.as_ref();
    let temporary = temporary_with_suffix(file_path, "tmp");
    let result = async {
        let mut file = File::create(&temporary).await?;
        file.write_all(content.as_ref()).await?;
        file.sync_all().await?;
        drop(file);
        replace_file(&temporary, file_path).await?;
        sync_dir(file_path.parent().unwrap_or_else(|| Path::new("."))).await
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&temporary).await;
    }
    result
}

pub async fn atomic_write(
    file_path: impl AsRef<Path>,
    content: impl AsRef<[u8]>,
    mode: Option<u32>,
) -> std::io::Result<()> {
    atomic_write_with_sync(file_path, content, mode, |file| {
        Box::pin(async move { file.sync_all().await })
    })
    .await
}

pub async fn atomic_write_with_sync<F>(
    file_path: impl AsRef<Path>,
    content: impl AsRef<[u8]>,
    mode: Option<u32>,
    sync: F,
) -> std::io::Result<()>
where
    F: for<'a> FnOnce(&'a File) -> BoxFuture<'a, std::io::Result<()>>,
{
    let file_path = file_path.as_ref();
    let suffix = format!(
        "tmp.{}.{}",
        std::process::id(),
        &Uuid::new_v4().simple().to_string()[..8]
    );
    let temporary = temporary_with_suffix(file_path, &suffix);
    let result = async {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        if let Some(mode) = mode {
            options.mode(mode);
        }
        #[cfg(not(unix))]
        let _ = mode;
        let mut file = options.open(&temporary).await?;
        file.write_all(content.as_ref()).await?;
        sync(&file).await?;
        drop(file);
        replace_file(&temporary, file_path).await
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&temporary).await;
    }
    result
}

fn temporary_with_suffix(file_path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut path = file_path.as_os_str().to_owned();
    path.push(".");
    path.push(suffix);
    path.into()
}

async fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    // Keep this as a single rename operation: on Windows, Rust implements rename
    // with MoveFileExW(MOVEFILE_REPLACE_EXISTING), so deleting `to` first creates
    // a crash- and reader-visible window in which neither version is available.
    fs::rename(from, to).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn durable_and_unique_atomic_writes_replace_content() {
        let directory = std::env::temp_dir().join(format!("kimi-fs-{}", Uuid::new_v4()));
        fs::create_dir(&directory).await.unwrap();
        let path = directory.join("state");
        write_file_atomic_durable(&path, b"first").await.unwrap();
        atomic_write(&path, b"second", Some(0o600)).await.unwrap();
        assert_eq!(fs::read(&path).await.unwrap(), b"second");
        fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn failed_sync_cleans_up_unique_temporary_file() {
        let directory = std::env::temp_dir().join(format!("kimi-fs-{}", Uuid::new_v4()));
        fs::create_dir(&directory).await.unwrap();
        let path = directory.join("state");
        let error = atomic_write_with_sync(&path, b"value", None, |_file| {
            Box::pin(async { Err(std::io::Error::other("sync failed")) })
        })
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "sync failed");
        assert!(
            fs::read_dir(&directory)
                .await
                .unwrap()
                .next_entry()
                .await
                .unwrap()
                .is_none()
        );
        fs::remove_dir_all(directory).await.unwrap();
    }
}
