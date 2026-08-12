use std::path::Path;

#[cfg(not(windows))]
use tokio::fs::File;

pub use kimi_code_fs::atomic_write;

// Original: packages/agent-core-v2/src/_base/utils/fs.ts, writeFileAtomicDurable().
pub async fn write_file_atomic_durable(
    file_path: impl AsRef<Path>,
    content: impl AsRef<[u8]>,
) -> std::io::Result<()> {
    kimi_code_fs::atomic_write_durable(file_path, content, None).await
}

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
