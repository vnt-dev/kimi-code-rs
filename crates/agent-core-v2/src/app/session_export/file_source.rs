//! Stable, bounded source file for session archive entries.
//! Original: `packages/agent-core-v2/src/app/sessionExport/file-source.ts`,
//! `openZipSource()`.
//!
//! Rust adaptation: Node returns a readable stream tied to its `FileHandle`.
//! `ZipSource::take_reader()` transfers that single stable Tokio file handle to
//! the archive writer; `close()` remains idempotent for failure cleanup before
//! that transfer.
use parking_lot::Mutex;
use std::{
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use tokio::fs::File;
use tokio_util::sync::CancellationToken;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZipSourceIdentity {
    pub device: u64,
    pub inode: u64,
}

#[derive(Debug)]
pub struct ZipSource {
    file: Mutex<Option<File>>,
    pub size: u64,
    pub modified: SystemTime,
    pub mode: u32,
    pub identity: ZipSourceIdentity,
    pub source_path: PathBuf,
}

pub async fn open_zip_source(
    source: &Path,
    cancellation: Option<&CancellationToken>,
) -> io::Result<ZipSource> {
    let file = File::open(source).await?;
    check(cancellation)?;
    let metadata = file.metadata().await?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a file: {}", source.display()),
        ));
    }
    let size = metadata.len();
    if size > MAX_SAFE_INTEGER {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("file is too large to export: {}", source.display()),
        ));
    }
    check(cancellation)?;

    Ok(ZipSource {
        file: Mutex::new(Some(file)),
        size,
        modified: metadata.modified()?,
        mode: file_mode(&metadata),
        identity: file_identity(&metadata),
        source_path: absolute_path(source)?,
    })
}

impl ZipSource {
    /// Transfers the bounded, already-open file handle to the archive writer.
    pub fn take_reader(&self) -> io::Result<File> {
        let mut file = self.lock_file()?;
        file.take().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "session export source is already closed",
            )
        })
    }

    /// Matches the TypeScript source's shared normal/error cleanup operation.
    pub async fn close(&self) -> io::Result<()> {
        drop(self.lock_file()?.take());
        Ok(())
    }

    fn lock_file(&self) -> io::Result<parking_lot::MutexGuard<'_, Option<File>>> {
        Ok(self.file.lock())
    }
}

fn check(cancellation: Option<&CancellationToken>) -> io::Result<()> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "session export cancelled",
        ))
    } else {
        Ok(())
    }
}

fn absolute_path(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

#[cfg(unix)]
fn file_mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    metadata.mode()
}

#[cfg(not(unix))]
fn file_mode(_: &std::fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn file_identity(metadata: &std::fs::Metadata) -> ZipSourceIdentity {
    use std::os::unix::fs::MetadataExt;
    ZipSourceIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(not(unix))]
fn file_identity(_: &std::fs::Metadata) -> ZipSourceIdentity {
    ZipSourceIdentity {
        device: 0,
        inode: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::fs;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn opens_a_stable_single_use_reader() {
        let path = std::env::temp_dir().join(format!("kimi-zip-source-{}", uuid::Uuid::new_v4()));
        fs::write(&path, b"archive content").await.unwrap();

        let source = open_zip_source(&path, None).await.unwrap();
        assert_eq!(source.size, 15);
        assert_eq!(source.source_path, absolute_path(&path).unwrap());
        let mut reader = source.take_reader().unwrap();
        let mut content = Vec::new();
        reader.read_to_end(&mut content).await.unwrap();
        assert_eq!(content, b"archive content");
        assert_eq!(
            source.take_reader().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        source.close().await.unwrap();
        fs::remove_file(path).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_a_cancelled_open_after_opening_the_file() {
        let path = std::env::temp_dir().join(format!("kimi-zip-source-{}", uuid::Uuid::new_v4()));
        fs::write(&path, b"x").await.unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error = open_zip_source(&path, Some(&cancellation))
            .await
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        fs::remove_file(path).await.unwrap();
    }
}
