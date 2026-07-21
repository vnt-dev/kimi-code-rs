use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use tokio::fs;
use uuid::Uuid;

use crate::utils::paths::{HomeDirectoryUnavailable, get_cache_dir};

const STALE_ARCHIVE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackArchive {
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
    pub fingerprint: String,
    pub file_count: usize,
    pub cleanup_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackArchivePath {
    pub archive_path: PathBuf,
    pub cleanup_dir: PathBuf,
}

#[derive(Debug)]
pub enum FeedbackArchiveError {
    Home(HomeDirectoryUnavailable),
    Io(std::io::Error),
    InvalidFilename,
}

impl fmt::Display for FeedbackArchiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Home(error) => error.fmt(formatter),
            Self::Io(error) => error.fmt(formatter),
            Self::InvalidFilename => {
                formatter.write_str("feedback archive filename must be a file name")
            }
        }
    }
}

impl Error for FeedbackArchiveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Home(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::InvalidFilename => None,
        }
    }
}

impl From<std::io::Error> for FeedbackArchiveError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

// Original: `src/feedback/archive.ts`, `createFeedbackArchivePath()`.
pub async fn create_feedback_archive_path(
    filename: &str,
) -> Result<FeedbackArchivePath, FeedbackArchiveError> {
    let cache_dir = get_cache_dir().map_err(FeedbackArchiveError::Home)?;
    create_feedback_archive_path_in(&cache_dir, filename).await
}

pub async fn create_feedback_archive_path_in(
    cache_dir: &Path,
    filename: &str,
) -> Result<FeedbackArchivePath, FeedbackArchiveError> {
    if Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
        != Some(filename)
    {
        return Err(FeedbackArchiveError::InvalidFilename);
    }
    let root = cache_dir.join("feedback-uploads");
    remove_stale_feedback_uploads_in(&root, SystemTime::now()).await?;
    fs::create_dir_all(&root).await?;
    let cleanup_dir = loop {
        let candidate = root.join(format!("upload-{}", Uuid::new_v4()));
        match fs::create_dir(&candidate).await {
            Ok(()) => break candidate,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    };
    Ok(FeedbackArchivePath {
        archive_path: cleanup_dir.join(filename),
        cleanup_dir,
    })
}

// Original: `removeStaleFeedbackUploads()`.
pub async fn remove_stale_feedback_uploads() -> Result<(), FeedbackArchiveError> {
    let root = get_cache_dir()
        .map_err(FeedbackArchiveError::Home)?
        .join("feedback-uploads");
    remove_stale_feedback_uploads_in(&root, SystemTime::now()).await
}

pub async fn remove_stale_feedback_uploads_in(
    root: &Path,
    now: SystemTime,
) -> Result<(), FeedbackArchiveError> {
    let mut entries = match fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        let file_type = match entry.file_type().await {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if !file_type.is_dir() && !file_type.is_symlink() {
            continue;
        }
        let metadata = match fs::metadata(entry.path()).await {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let modified = match metadata.modified() {
            Ok(modified) => modified,
            Err(_) => continue,
        };
        if now.duration_since(modified).unwrap_or_default() < STALE_ARCHIVE_MAX_AGE {
            continue;
        }
        if file_type.is_symlink() {
            let _ = fs::remove_file(entry.path()).await;
        } else {
            let _ = fs::remove_dir_all(entry.path()).await;
        }
    }
    Ok(())
}

pub async fn cleanup_feedback_archive(directory: &Path) {
    let _ = fs::remove_dir_all(directory).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("kimi-feedback-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn creates_unique_cleanup_directory_and_archive_filename() {
        let temp = TempDir::new();
        let first = create_feedback_archive_path_in(&temp.0, "session.zip")
            .await
            .expect("first path");
        let second = create_feedback_archive_path_in(&temp.0, "session.zip")
            .await
            .expect("second path");
        assert_ne!(first.cleanup_dir, second.cleanup_dir);
        assert_eq!(
            first
                .archive_path
                .file_name()
                .and_then(|name| name.to_str()),
            Some("session.zip")
        );
        assert!(first.cleanup_dir.is_dir());
    }

    #[tokio::test]
    async fn rejects_paths_instead_of_accepting_filename_traversal() {
        let temp = TempDir::new();
        for filename in ["../secret.zip", "nested/session.zip", ""] {
            assert!(matches!(
                create_feedback_archive_path_in(&temp.0, filename).await,
                Err(FeedbackArchiveError::InvalidFilename)
            ));
        }
    }

    #[tokio::test]
    async fn stale_cleanup_removes_directories_but_keeps_regular_files() {
        let temp = TempDir::new();
        let root = temp.0.join("feedback-uploads");
        let stale = root.join("upload-old");
        fs::create_dir_all(&stale).await.expect("stale dir");
        fs::write(root.join("keep.log"), b"keep")
            .await
            .expect("file");
        let modified = fs::metadata(&stale)
            .await
            .expect("metadata")
            .modified()
            .expect("modified");
        remove_stale_feedback_uploads_in(
            &root,
            modified + STALE_ARCHIVE_MAX_AGE + Duration::from_secs(1),
        )
        .await
        .expect("cleanup");
        assert!(!stale.exists());
        assert!(root.join("keep.log").exists());
    }

    #[tokio::test]
    async fn missing_stale_root_is_a_noop() {
        let temp = TempDir::new();
        remove_stale_feedback_uploads_in(&temp.0.join("missing"), SystemTime::now())
            .await
            .expect("missing root");
    }
}
