use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::UNIX_EPOCH,
};

use sha2::{Digest, Sha256};
use tokio::{fs, process::Command, sync::Notify};

use super::{
    filter::{
        DEFAULT_MAX_ARCHIVE_SIZE, DEFAULT_MAX_FILE_SIZE, DEFAULT_MAX_FILES, is_ignored_dir_name,
        is_sensitive_path,
    },
    types::{
        FeedbackCodebaseFile, FeedbackCodebaseLimitExceeded, FeedbackCodebaseLimitReason,
        FeedbackCodebaseScanResult,
    },
};

const GIT_LIST_MAX_BUFFER: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ScanCodebaseLimits {
    pub max_files: usize,
    pub max_file_size: u64,
    pub max_archive_size: u64,
}

impl Default for ScanCodebaseLimits {
    fn default() -> Self {
        Self {
            max_files: DEFAULT_MAX_FILES,
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            max_archive_size: DEFAULT_MAX_ARCHIVE_SIZE,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScanCancellation {
    aborted: Arc<AtomicBool>,
    notification: Arc<Notify>,
}

impl ScanCancellation {
    pub fn abort(&self) {
        self.aborted.store(true, Ordering::SeqCst);
        self.notification.notify_waiters();
    }

    pub fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::SeqCst)
    }

    async fn cancelled(&self) {
        let notification = self.notification.notified();
        if self.is_aborted() {
            return;
        }
        notification.await;
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScanCodebaseOptions {
    pub limits: ScanCodebaseLimits,
    pub cancellation: Option<ScanCancellation>,
}

#[derive(Debug)]
pub enum ScanCodebaseError {
    Aborted,
    Io(std::io::Error),
    GitListFailed(String),
    GitOutputTooLarge,
}

impl fmt::Display for ScanCodebaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Aborted => formatter.write_str("Codebase scan aborted."),
            Self::Io(error) => error.fmt(formatter),
            Self::GitListFailed(message) => write!(formatter, "git ls-files failed: {message}"),
            Self::GitOutputTooLarge => formatter.write_str("git ls-files output exceeded 64 MiB"),
        }
    }
}

impl Error for ScanCodebaseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Aborted | Self::GitListFailed(_) | Self::GitOutputTooLarge => None,
        }
    }
}

impl From<std::io::Error> for ScanCodebaseError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

struct CollectedFiles {
    files: Vec<FeedbackCodebaseFile>,
    exceeds_limit: Option<FeedbackCodebaseLimitExceeded>,
}

// Original: `src/feedback/codebase/scanner.ts`, `scanCodebase()`.
pub async fn scan_codebase(
    root_input: &Path,
    options: ScanCodebaseOptions,
) -> Result<FeedbackCodebaseScanResult, ScanCodebaseError> {
    let root = absolute_path(root_input)?;
    throw_if_aborted(options.cancellation.as_ref())?;
    let used_git_ignore = is_inside_git_work_tree(&root).await;
    let collected = if used_git_ignore {
        scan_with_git(&root, &options.limits, options.cancellation.as_ref()).await?
    } else {
        scan_without_filter(&root, &options.limits, options.cancellation.as_ref()).await?
    };
    let mut files = collected.files;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let fingerprint = fingerprint_files(&files);

    Ok(FeedbackCodebaseScanResult {
        root,
        files,
        fingerprint,
        used_git_ignore,
        exceeds_limit: collected.exceeds_limit,
    })
}

fn absolute_path(path: &Path) -> Result<PathBuf, std::io::Error> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

async fn is_inside_git_work_tree(root: &Path) -> bool {
    Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .await
        .is_ok_and(|output| {
            output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true"
        })
}

async fn scan_with_git(
    root: &Path,
    limits: &ScanCodebaseLimits,
    cancellation: Option<&ScanCancellation>,
) -> Result<CollectedFiles, ScanCodebaseError> {
    let mut command = Command::new("git");
    command
        .args(["-C"])
        .arg(root)
        .args(["ls-files", "-co", "--exclude-standard", "-z"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command.spawn()?;
    let output = if let Some(cancellation) = cancellation {
        tokio::select! {
            output = child.wait_with_output() => output?,
            () = cancellation.cancelled() => return Err(ScanCodebaseError::Aborted),
        }
    } else {
        child.wait_with_output().await?
    };
    if !output.status.success() {
        return Err(ScanCodebaseError::GitListFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    if output.stdout.len() > GIT_LIST_MAX_BUFFER {
        return Err(ScanCodebaseError::GitOutputTooLarge);
    }

    throw_if_aborted(cancellation)?;
    let relative_paths = String::from_utf8_lossy(&output.stdout);
    let mut files = Vec::new();
    let mut exceeds_limit = None;
    let mut total_size = 0_u64;
    for relative_path in relative_paths.split('\0').filter(|path| !path.is_empty()) {
        throw_if_aborted(cancellation)?;
        if files.len() >= limits.max_files {
            exceeds_limit = Some(file_count_limit(limits.max_files));
            break;
        }
        if is_sensitive_path(relative_path) {
            continue;
        }
        if let Some(file) = stat_file(root, relative_path).await {
            if file.size > limits.max_file_size {
                continue;
            }
            if total_size.saturating_add(file.size) > limits.max_archive_size {
                exceeds_limit = Some(total_size_limit(limits.max_archive_size));
                break;
            }
            total_size += file.size;
            files.push(file);
        }
    }
    Ok(CollectedFiles {
        files,
        exceeds_limit,
    })
}

async fn scan_without_filter(
    root: &Path,
    limits: &ScanCodebaseLimits,
    cancellation: Option<&ScanCancellation>,
) -> Result<CollectedFiles, ScanCodebaseError> {
    let mut files = Vec::new();
    let mut exceeds_limit = None;
    let mut total_size = 0_u64;
    let mut directories = vec![fs::read_dir(root).await?];

    'walk: while let Some(directory) = directories.last_mut() {
        throw_if_aborted(cancellation)?;
        let Some(entry) = directory.next_entry().await? else {
            directories.pop();
            continue;
        };
        if files.len() >= limits.max_files {
            exceeds_limit = Some(file_count_limit(limits.max_files));
            break;
        }
        let file_type = entry.file_type().await?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if is_ignored_dir_name(&entry.file_name().to_string_lossy()) {
                continue;
            }
            directories.push(fs::read_dir(entry.path()).await?);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let relative_path = path_to_posix(entry.path().strip_prefix(root).unwrap_or(&entry.path()));
        if is_sensitive_path(&relative_path) {
            continue;
        }
        if let Some(file) = stat_file(root, &relative_path).await {
            if file.size > limits.max_file_size {
                continue;
            }
            if total_size.saturating_add(file.size) > limits.max_archive_size {
                exceeds_limit = Some(total_size_limit(limits.max_archive_size));
                break 'walk;
            }
            total_size += file.size;
            files.push(file);
        }
    }

    Ok(CollectedFiles {
        files,
        exceeds_limit,
    })
}

async fn stat_file(root: &Path, relative_path: &str) -> Option<FeedbackCodebaseFile> {
    let absolute_path = root.join(relative_path.replace('/', std::path::MAIN_SEPARATOR_STR));
    let metadata = fs::symlink_metadata(&absolute_path).await.ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return None;
    }
    let mtime_ms = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    Some(FeedbackCodebaseFile {
        path: relative_path.replace('\\', "/"),
        absolute_path,
        size: metadata.len(),
        mtime_ms,
    })
}

fn throw_if_aborted(cancellation: Option<&ScanCancellation>) -> Result<(), ScanCodebaseError> {
    if cancellation.is_some_and(ScanCancellation::is_aborted) {
        Err(ScanCodebaseError::Aborted)
    } else {
        Ok(())
    }
}

fn fingerprint_files(files: &[FeedbackCodebaseFile]) -> String {
    let mut hash = Sha256::new();
    for file in files {
        hash.update(file.path.as_bytes());
        hash.update(b"\0");
        hash.update(file.size.to_string().as_bytes());
        hash.update(b"\0");
        hash.update(file.mtime_ms.to_string().as_bytes());
        hash.update(b"\n");
    }
    let digest = hash.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn path_to_posix(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn file_count_limit(limit: usize) -> FeedbackCodebaseLimitExceeded {
    FeedbackCodebaseLimitExceeded {
        reason: FeedbackCodebaseLimitReason::FileCount,
        limit: limit.try_into().unwrap_or(u64::MAX),
    }
}

fn total_size_limit(limit: u64) -> FeedbackCodebaseLimitExceeded {
    FeedbackCodebaseLimitExceeded {
        reason: FeedbackCodebaseLimitReason::TotalSize,
        limit,
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let path = std::env::temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        fn write(&self, relative: &str, content: &[u8]) {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("parent");
            }
            std::fs::write(path, content).expect("write");
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn scan(root: &Path) -> FeedbackCodebaseScanResult {
        scan_codebase(root, ScanCodebaseOptions::default())
            .await
            .expect("scan")
    }

    #[tokio::test]
    async fn rejects_an_already_aborted_scan() {
        let temp = TempDir::new("feedback-scan-aborted");
        let cancellation = ScanCancellation::default();
        cancellation.abort();
        let error = scan_codebase(
            &temp.0,
            ScanCodebaseOptions {
                cancellation: Some(cancellation),
                ..ScanCodebaseOptions::default()
            },
        )
        .await
        .expect_err("aborted");
        assert!(matches!(error, ScanCodebaseError::Aborted));
        assert_eq!(error.to_string(), "Codebase scan aborted.");
    }

    #[tokio::test]
    async fn filters_ignored_and_sensitive_paths_without_git() {
        let temp = TempDir::new("feedback-scan-filter");
        temp.write("node_modules/pkg/index.js", b"module.exports = 1;\n");
        temp.write("dist/bundle.js", b"built\n");
        temp.write(".ssh/config", b"Host *\n");
        temp.write(".env.production", b"SECRET=1\n");
        temp.write("keep.ts", b"export const keep = 1;\n");

        let result = scan(&temp.0).await;

        assert!(!result.used_git_ignore);
        assert_eq!(
            result
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["keep.ts"]
        );
    }

    #[tokio::test]
    async fn git_scan_filters_sensitive_and_vanished_tracked_files() {
        let temp = TempDir::new("feedback-scan-git");
        temp.write(".env", b"SECRET=1\n");
        temp.write("app.ts", b"export const app = 1;\n");
        temp.write("deleted.ts", b"gone\n");
        let status = std::process::Command::new("git")
            .arg("init")
            .current_dir(&temp.0)
            .status()
            .expect("git init");
        assert!(status.success());
        let status = std::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(&temp.0)
            .status()
            .expect("git add");
        assert!(status.success());
        std::fs::remove_file(temp.0.join("deleted.ts")).expect("remove tracked file");

        let result = scan(&temp.0).await;

        assert!(result.used_git_ignore);
        assert_eq!(
            result
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["app.ts"]
        );
    }

    #[tokio::test]
    async fn skips_oversized_files_and_reports_file_count_limit() {
        let temp = TempDir::new("feedback-scan-limits");
        temp.write("a.txt", b"a");
        temp.write("b.txt", b"b");
        temp.write("c.bin", &[0; 256]);
        temp.write("d.txt", b"d");
        let result = scan_codebase(
            &temp.0,
            ScanCodebaseOptions {
                limits: ScanCodebaseLimits {
                    max_files: 2,
                    max_file_size: 128,
                    max_archive_size: DEFAULT_MAX_ARCHIVE_SIZE,
                },
                cancellation: None,
            },
        )
        .await
        .expect("scan");
        assert_eq!(result.files.len(), 2);
        assert_eq!(
            result.exceeds_limit,
            Some(FeedbackCodebaseLimitExceeded {
                reason: FeedbackCodebaseLimitReason::FileCount,
                limit: 2,
            })
        );
    }

    #[tokio::test]
    async fn reports_cumulative_size_limit_and_stable_fingerprint() {
        let temp = TempDir::new("feedback-scan-size");
        temp.write("c.txt", &[b'c'; 100]);
        temp.write("a.txt", &[b'a'; 100]);
        temp.write("b.txt", &[b'b'; 100]);
        let options = || ScanCodebaseOptions {
            limits: ScanCodebaseLimits {
                max_archive_size: 250,
                ..ScanCodebaseLimits::default()
            },
            cancellation: None,
        };

        let first = scan_codebase(&temp.0, options()).await.expect("first");
        let second = scan_codebase(&temp.0, options()).await.expect("second");

        assert_eq!(first.files.len(), 2);
        assert_eq!(first.fingerprint, second.fingerprint);
        assert_eq!(
            first.exceeds_limit,
            Some(FeedbackCodebaseLimitExceeded {
                reason: FeedbackCodebaseLimitReason::TotalSize,
                limit: 250,
            })
        );
    }
}
