use std::{
    error::Error,
    fmt,
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::feedback::archive::FeedbackArchive;

use super::types::FeedbackCodebaseScanResult;

#[derive(Debug, Clone)]
struct PackageEntry {
    absolute_path: PathBuf,
    archive_path: String,
    size: u64,
    mtime_ms: u64,
}

#[derive(Debug)]
pub enum PackageCodebaseError {
    EmptyArchive,
    Io(std::io::Error),
    Zip(zip::result::ZipError),
    Task(tokio::task::JoinError),
}

impl fmt::Display for PackageCodebaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyArchive => formatter.write_str("Cannot package an empty feedback archive."),
            Self::Io(error) => error.fmt(formatter),
            Self::Zip(error) => error.fmt(formatter),
            Self::Task(error) => write!(formatter, "feedback archive task failed: {error}"),
        }
    }
}

impl Error for PackageCodebaseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Zip(error) => Some(error),
            Self::Task(error) => Some(error),
            Self::EmptyArchive => None,
        }
    }
}

impl From<std::io::Error> for PackageCodebaseError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<zip::result::ZipError> for PackageCodebaseError {
    fn from(error: zip::result::ZipError) -> Self {
        Self::Zip(error)
    }
}

// Original: `src/feedback/codebase/packager.ts`, `packageCodebase()`.
//
// Rust adaptation: the synchronous ZIP encoder and file reads run on Tokio's
// blocking pool. The async boundary still removes any partial archive before
// returning an encoding, source-file, or task error.
pub async fn package_codebase(
    scan: &FeedbackCodebaseScanResult,
    archive_path: &Path,
) -> Result<FeedbackArchive, PackageCodebaseError> {
    let entries = scan
        .files
        .iter()
        .map(|file| PackageEntry {
            absolute_path: file.absolute_path.clone(),
            archive_path: file.path.clone(),
            size: file.size,
            mtime_ms: file.mtime_ms,
        })
        .collect::<Vec<_>>();
    package_entries(entries, archive_path.to_path_buf()).await
}

async fn package_entries(
    entries: Vec<PackageEntry>,
    archive_path: PathBuf,
) -> Result<FeedbackArchive, PackageCodebaseError> {
    if entries.is_empty() {
        return Err(PackageCodebaseError::EmptyArchive);
    }
    if let Some(parent) = archive_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let cleanup_path = archive_path.clone();
    let result =
        tokio::task::spawn_blocking(move || package_entries_blocking(&entries, archive_path))
            .await
            .map_err(PackageCodebaseError::Task)
            .and_then(|result| result);
    if result.is_err() {
        let _ = tokio::fs::remove_file(cleanup_path).await;
    }
    result
}

fn package_entries_blocking(
    entries: &[PackageEntry],
    archive_path: PathBuf,
) -> Result<FeedbackArchive, PackageCodebaseError> {
    let output = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&archive_path)?;
    let mut zip = ZipWriter::new(output);
    for entry in entries {
        let modified =
            DateTime::<Utc>::from_timestamp_millis(entry.mtime_ms.try_into().unwrap_or(i64::MAX))
                .map(|timestamp| timestamp.naive_utc())
                .and_then(|timestamp| zip::DateTime::try_from(timestamp).ok())
                .unwrap_or(zip::DateTime::DEFAULT);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(modified)
            .unix_permissions(0o644)
            .large_file(entry.size > u64::from(u32::MAX));
        zip.start_file(&entry.archive_path, options)?;
        let mut source = File::open(&entry.absolute_path)?;
        std::io::copy(&mut source, &mut zip)?;
    }
    let mut output = zip.finish()?;
    output.flush()?;
    let size = output.metadata()?.len();
    output.seek(SeekFrom::Start(0))?;
    let sha256 = hash_reader(&mut output)?;

    Ok(FeedbackArchive {
        path: archive_path,
        size,
        sha256,
        fingerprint: fingerprint_entries(entries),
        file_count: entries.len(),
        cleanup_dir: None,
    })
}

fn fingerprint_entries(entries: &[PackageEntry]) -> String {
    let mut hash = Sha256::new();
    for entry in entries {
        hash.update(entry.archive_path.as_bytes());
        hash.update(b"\0");
        hash.update(entry.size.to_string().as_bytes());
        hash.update(b"\0");
        hash.update(entry.mtime_ms.to_string().as_bytes());
        hash.update(b"\n");
    }
    encode_hex(&hash.finalize())
}

fn hash_reader(reader: &mut impl Read) -> Result<String, std::io::Error> {
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(encode_hex(&hash.finalize()))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use uuid::Uuid;

    use super::*;
    use crate::feedback::codebase::types::{FeedbackCodebaseFile, FeedbackCodebaseScanResult};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("feedback-package-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn file(root: &Path, relative: &str, content: &[u8]) -> FeedbackCodebaseFile {
        let absolute_path = root.join(relative);
        if let Some(parent) = absolute_path.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(&absolute_path, content).expect("write");
        let metadata = std::fs::metadata(&absolute_path).expect("metadata");
        let mtime_ms = metadata
            .modified()
            .expect("modified")
            .duration_since(UNIX_EPOCH)
            .expect("timestamp")
            .as_millis()
            .try_into()
            .expect("milliseconds");
        FeedbackCodebaseFile {
            path: relative.replace('\\', "/"),
            absolute_path,
            size: metadata.len(),
            mtime_ms,
        }
    }

    fn scan(root: &Path, files: Vec<FeedbackCodebaseFile>) -> FeedbackCodebaseScanResult {
        FeedbackCodebaseScanResult {
            root: root.to_path_buf(),
            files,
            fingerprint: "scan-fingerprint".to_owned(),
            used_git_ignore: false,
            exceeds_limit: None,
        }
    }

    #[tokio::test]
    async fn rejects_empty_archives_without_creating_output() {
        let temp = TempDir::new();
        let archive_path = temp.0.join("nested/repo.zip");
        let error = package_codebase(&scan(&temp.0, Vec::new()), &archive_path)
            .await
            .expect_err("empty archive");
        assert!(matches!(error, PackageCodebaseError::EmptyArchive));
        assert!(!archive_path.exists());
    }

    #[tokio::test]
    async fn packages_files_at_zip_root_with_content_permissions_and_metadata() {
        let temp = TempDir::new();
        let first = file(&temp.0, "src/main.rs", b"fn main() {}\n");
        let second = file(&temp.0, "README.md", b"# demo\n");
        let expected_fingerprint = fingerprint_entries(&[
            PackageEntry {
                absolute_path: first.absolute_path.clone(),
                archive_path: first.path.clone(),
                size: first.size,
                mtime_ms: first.mtime_ms,
            },
            PackageEntry {
                absolute_path: second.absolute_path.clone(),
                archive_path: second.path.clone(),
                size: second.size,
                mtime_ms: second.mtime_ms,
            },
        ]);
        let archive_path = temp.0.join("out/repo.zip");

        let archive = package_codebase(&scan(&temp.0, vec![first, second]), &archive_path)
            .await
            .expect("package");

        assert_eq!(archive.path, archive_path);
        assert_eq!(archive.file_count, 2);
        assert_eq!(archive.fingerprint, expected_fingerprint);
        assert_eq!(archive.sha256.len(), 64);
        assert_eq!(
            archive.size,
            std::fs::metadata(&archive.path).expect("zip").len()
        );
        let input = File::open(&archive.path).expect("archive");
        let mut zip = zip::ZipArchive::new(input).expect("read zip");
        let mut main = zip.by_name("src/main.rs").expect("main entry");
        let mut content = String::new();
        main.read_to_string(&mut content).expect("content");
        assert_eq!(content, "fn main() {}\n");
        assert_eq!(main.unix_mode(), Some(0o100644));
        drop(main);
        assert!(zip.by_name("README.md").is_ok());
    }

    #[tokio::test]
    async fn removes_partial_archive_when_a_scanned_source_vanishes() {
        let temp = TempDir::new();
        let present = file(&temp.0, "present.txt", b"present");
        let missing = FeedbackCodebaseFile {
            path: "missing.txt".to_owned(),
            absolute_path: temp.0.join("missing.txt"),
            size: 7,
            mtime_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_millis()
                .try_into()
                .expect("milliseconds"),
        };
        let archive_path = temp.0.join("out/repo.zip");

        let error = package_codebase(&scan(&temp.0, vec![present, missing]), &archive_path)
            .await
            .expect_err("missing source");

        assert!(matches!(error, PackageCodebaseError::Io(_)));
        assert!(!archive_path.exists());
    }
}
