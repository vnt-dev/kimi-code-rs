//! Session diagnostic ZIP writer.
//! Original: `packages/agent-core-v2/src/app/sessionExport/zip.ts`,
//! `collectFilesRecursive()`, `writeExportZip()`, and conflict helpers.
//!
//! Rust adaptation: the `zip` crate is synchronous, so every archive-writing
//! step runs in `spawn_blocking`; file discovery, opening, cancellation, and
//! rename/cleanup stay asynchronous and preserve the source ordering.
use std::{
    io,
    io::Read,
    path::{Path, PathBuf},
    time::SystemTime,
};

use chrono::{Datelike, Timelike};
use thiserror::Error;
use tokio::{fs, task};
use tokio_util::sync::CancellationToken;
use zip::{ZipWriter, write::SimpleFileOptions};

use super::{ExportSessionManifest, ZipSource, ZipSourceIdentity, open_zip_source};

#[derive(Debug)]
pub enum ExtraZipEntry {
    Source { source: ZipSource, target: String },
    Data { data: Vec<u8>, target: String },
}

#[derive(Debug)]
pub enum SessionZipEntry {
    Path(PathBuf),
    Source { path: PathBuf, source: ZipSource },
}

#[derive(Debug)]
pub struct WriteExportZipArgs {
    pub output_path: PathBuf,
    pub manifest: ExportSessionManifest,
    pub session_dir: PathBuf,
    pub session_files: Vec<SessionZipEntry>,
    pub extra_entries: Vec<ExtraZipEntry>,
    pub cancellation: Option<CancellationToken>,
    pub max_archive_bytes: Option<u64>,
}

#[derive(Debug, Error)]
pub enum ExportZipError {
    #[error("session export output conflicts with selected source \"{path}\"")]
    OutputConflict { path: String },

    #[error(
        "session export exceeds the {max_archive_bytes} byte archive limit (wrote {archive_bytes} bytes)"
    )]
    TooLarge {
        archive_bytes: u64,
        max_archive_bytes: u64,
    },

    #[error("session export was cancelled")]
    Cancelled,

    #[error(transparent)]
    Io(#[from] io::Error),

    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),

    #[error(transparent)]
    Serialize(#[from] serde_json::Error),

    #[error("session export archive task failed: {0}")]
    Join(#[from] task::JoinError),
}

type BlockingWriter = ZipWriter<std::fs::File>;

pub async fn collect_files_recursive(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let mut entries = match fs::read_dir(directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        while let Some(entry) = entries.next_entry().await? {
            let kind = entry.file_type().await?;
            if kind.is_file() {
                files.push(entry.path());
            } else if kind.is_dir() {
                directories.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

pub async fn write_export_zip(mut args: WriteExportZipArgs) -> Result<Vec<String>, ExportZipError> {
    check(args.cancellation.as_ref())?;
    if let Some(path) = find_conflicting_source(&args).await? {
        return Err(ExportZipError::OutputConflict { path });
    }

    let output_parent = args.output_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(output_parent).await?;
    check(args.cancellation.as_ref())?;
    let temp_dir = create_temp_dir(output_parent).await?;
    let temp_output = temp_dir.join("archive.zip");

    let result = write_export_zip_in_temp(&mut args, &temp_output).await;
    let cleanup_result = fs::remove_dir_all(&temp_dir).await;
    match (result, cleanup_result) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(ExportZipError::Io(error)),
        (Ok(entries), Ok(())) => Ok(entries),
    }
}

async fn write_export_zip_in_temp(
    args: &mut WriteExportZipArgs,
    temp_output: &Path,
) -> Result<Vec<String>, ExportZipError> {
    let manifest = serde_json::to_vec_pretty(&args.manifest)?;
    let output = temp_output.to_path_buf();
    let mut writer = task::spawn_blocking(move || -> Result<BlockingWriter, ExportZipError> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?;
        let mut writer = ZipWriter::new(file);
        writer.start_file("manifest.json", SimpleFileOptions::default())?;
        use std::io::Write;
        writer.write_all(&manifest)?;
        Ok(writer)
    })
    .await??;

    let session_dir = std::path::absolute(&args.session_dir)?;
    let mut entries = vec!["manifest.json".to_owned()];
    for entry in std::mem::take(&mut args.session_files) {
        check(args.cancellation.as_ref())?;
        let (path, source) = open_session_entry(entry, args.cancellation.as_ref()).await?;
        let target = session_target(&session_dir, &path)?;
        writer = write_source(writer, target.clone(), source, args.cancellation.as_ref()).await?;
        entries.push(target);
    }

    for entry in std::mem::take(&mut args.extra_entries) {
        check(args.cancellation.as_ref())?;
        match entry {
            ExtraZipEntry::Data { data, target } => {
                writer = write_data(writer, target.clone(), data).await?;
                entries.push(target);
            }
            ExtraZipEntry::Source { source, target } => {
                writer = write_source(writer, target.clone(), source, args.cancellation.as_ref())
                    .await?;
                entries.push(target);
            }
        }
    }

    task::spawn_blocking(move || writer.finish()).await??;
    check(args.cancellation.as_ref())?;
    let archive_bytes = fs::metadata(temp_output).await?.len();
    if let Some(max_archive_bytes) = args.max_archive_bytes
        && archive_bytes > max_archive_bytes
    {
        return Err(ExportZipError::TooLarge {
            archive_bytes,
            max_archive_bytes,
        });
    }
    fs::rename(temp_output, &args.output_path).await?;
    Ok(entries)
}

async fn open_session_entry(
    entry: SessionZipEntry,
    cancellation: Option<&CancellationToken>,
) -> Result<(PathBuf, ZipSource), ExportZipError> {
    match entry {
        SessionZipEntry::Path(path) => {
            let source = open_zip_source(&path, cancellation).await?;
            Ok((path, source))
        }
        SessionZipEntry::Source { path, source } => Ok((path, source)),
    }
}

async fn write_source(
    writer: BlockingWriter,
    target: String,
    source: ZipSource,
    cancellation: Option<&CancellationToken>,
) -> Result<BlockingWriter, ExportZipError> {
    check(cancellation)?;
    let options = source_options(&source);
    let reader = source.take_reader()?.into_std().await;
    let cancellation = cancellation.cloned();
    task::spawn_blocking(move || -> Result<BlockingWriter, ExportZipError> {
        let mut writer = writer;
        writer.start_file(target, options)?;
        copy_with_cancellation(
            &mut reader.take(source.size),
            &mut writer,
            cancellation.as_ref(),
        )?;
        Ok(writer)
    })
    .await?
}

async fn write_data(
    writer: BlockingWriter,
    target: String,
    data: Vec<u8>,
) -> Result<BlockingWriter, ExportZipError> {
    task::spawn_blocking(move || -> Result<BlockingWriter, ExportZipError> {
        let mut writer = writer;
        writer.start_file(target, SimpleFileOptions::default())?;
        use std::io::Write;
        writer.write_all(&data)?;
        Ok(writer)
    })
    .await?
}

fn copy_with_cancellation(
    reader: &mut std::io::Take<std::fs::File>,
    writer: &mut BlockingWriter,
    cancellation: Option<&CancellationToken>,
) -> Result<(), ExportZipError> {
    use std::io::{Read, Write};
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        check(cancellation)?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        writer.write_all(&buffer[..read])?;
    }
}

fn source_options(source: &ZipSource) -> SimpleFileOptions {
    let options = SimpleFileOptions::default().unix_permissions(source.mode);
    zip_datetime(source.modified).map_or(options, |time| options.last_modified_time(time))
}

fn zip_datetime(time: SystemTime) -> Option<zip::DateTime> {
    let time: chrono::DateTime<chrono::Utc> = time.into();
    zip::DateTime::from_date_and_time(
        u16::try_from(time.year()).ok()?,
        u8::try_from(time.month()).ok()?,
        u8::try_from(time.day()).ok()?,
        u8::try_from(time.hour()).ok()?,
        u8::try_from(time.minute()).ok()?,
        u8::try_from(time.second()).ok()?,
    )
    .ok()
}

fn session_target(session_dir: &Path, path: &Path) -> Result<String, ExportZipError> {
    let path = std::path::absolute(path)?;
    let target = path.strip_prefix(session_dir).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "session export source is outside the session directory: {}",
                path.display()
            ),
        )
    })?;
    Ok(target.to_string_lossy().replace('\\', "/"))
}

async fn find_conflicting_source(
    args: &WriteExportZipArgs,
) -> Result<Option<String>, ExportZipError> {
    let output_path = std::path::absolute(&args.output_path)?;
    for entry in &args.session_files {
        check(args.cancellation.as_ref())?;
        let path = session_entry_path(entry);
        if std::path::absolute(path)? == output_path {
            return Ok(Some(path.display().to_string()));
        }
    }
    for entry in &args.extra_entries {
        if let ExtraZipEntry::Source { source, target } = entry
            && std::path::absolute(&source.source_path)? == output_path
        {
            return Ok(Some(target.clone()));
        }
    }

    let Some(output_identity) = stat_existing(&output_path).await? else {
        return Ok(None);
    };
    for entry in &args.session_files {
        check(args.cancellation.as_ref())?;
        let input = match entry {
            SessionZipEntry::Path(path) => stat_existing(path).await?,
            SessionZipEntry::Source { source, .. } => Some(source.identity),
        };
        if input.is_some_and(|input| same_file(output_identity, input)) {
            return Ok(Some(session_entry_path(entry).display().to_string()));
        }
    }
    for entry in &args.extra_entries {
        check(args.cancellation.as_ref())?;
        if let ExtraZipEntry::Source { source, target } = entry
            && same_file(output_identity, source.identity)
        {
            return Ok(Some(target.clone()));
        }
    }
    Ok(None)
}

async fn stat_existing(path: &Path) -> io::Result<Option<ZipSourceIdentity>> {
    match fs::metadata(path).await {
        Ok(metadata) => Ok(Some(file_identity(&metadata))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn same_file(left: ZipSourceIdentity, right: ZipSourceIdentity) -> bool {
    left.inode != 0 && left.device == right.device && left.inode == right.inode
}

fn session_entry_path(entry: &SessionZipEntry) -> &Path {
    match entry {
        SessionZipEntry::Path(path) | SessionZipEntry::Source { path, .. } => path,
    }
}

async fn create_temp_dir(parent: &Path) -> io::Result<PathBuf> {
    for _ in 0..16 {
        let path = parent.join(format!(".kimi-session-export-{}", uuid::Uuid::new_v4()));
        match fs::create_dir(&path).await {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate session export temporary directory",
    ))
}

fn check(cancellation: Option<&CancellationToken>) -> Result<(), ExportZipError> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        Err(ExportZipError::Cancelled)
    } else {
        Ok(())
    }
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
    use std::io::Read;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("kimi-export-zip-{}", uuid::Uuid::new_v4()))
    }

    fn manifest() -> ExportSessionManifest {
        ExportSessionManifest {
            session_id: "session".to_owned(),
            exported_at: "2026-01-01T00:00:00.000Z".to_owned(),
            kimi_code_version: "1.0.0".to_owned(),
            wire_protocol_version: "1".to_owned(),
            os: "linux".to_owned(),
            nodejs_version: "v22.0.0".to_owned(),
            session_first_activity: None,
            session_last_activity: None,
            title: None,
            workspace_dir: None,
            session_log_path: None,
            global_log_path: None,
            web_log_path: None,
            install_source: None,
            shell_env: None,
        }
    }

    #[tokio::test]
    async fn collects_sorted_regular_files_and_writes_archive() {
        let root = temp_dir();
        let session_dir = root.join("session");
        fs::create_dir_all(session_dir.join("nested"))
            .await
            .unwrap();
        fs::write(session_dir.join("z.txt"), b"z").await.unwrap();
        fs::write(session_dir.join("nested").join("a.txt"), b"a")
            .await
            .unwrap();
        let files = collect_files_recursive(&session_dir).await.unwrap();
        assert_eq!(
            files,
            vec![
                session_dir.join("nested").join("a.txt"),
                session_dir.join("z.txt")
            ]
        );

        let output = root.join("session.zip");
        let entries = write_export_zip(WriteExportZipArgs {
            output_path: output.clone(),
            manifest: manifest(),
            session_dir: session_dir.clone(),
            session_files: files.into_iter().map(SessionZipEntry::Path).collect(),
            extra_entries: vec![ExtraZipEntry::Data {
                data: b"extra".to_vec(),
                target: "extra.txt".to_owned(),
            }],
            cancellation: None,
            max_archive_bytes: None,
        })
        .await
        .unwrap();

        assert_eq!(
            entries,
            ["manifest.json", "nested/a.txt", "z.txt", "extra.txt"]
        );
        let output = output.clone();
        let names = task::spawn_blocking(move || {
            let mut archive = zip::ZipArchive::new(std::fs::File::open(output).unwrap()).unwrap();
            let mut manifest = String::new();
            archive
                .by_name("manifest.json")
                .unwrap()
                .read_to_string(&mut manifest)
                .unwrap();
            assert!(manifest.contains("\"sessionId\": \"session\""));
            (0..archive.len())
                .map(|index| archive.by_index(index).unwrap().name().to_owned())
                .collect::<Vec<_>>()
        })
        .await
        .unwrap();
        assert_eq!(
            names,
            ["manifest.json", "nested/a.txt", "z.txt", "extra.txt"]
        );
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_output_path_that_is_a_selected_source() {
        let root = temp_dir();
        fs::create_dir_all(&root).await.unwrap();
        let output = root.join("archive.zip");
        fs::write(&output, b"existing").await.unwrap();
        let error = write_export_zip(WriteExportZipArgs {
            output_path: output.clone(),
            manifest: manifest(),
            session_dir: root.clone(),
            session_files: vec![SessionZipEntry::Path(output.clone())],
            extra_entries: Vec::new(),
            cancellation: None,
            max_archive_bytes: None,
        })
        .await
        .unwrap_err();
        assert!(matches!(error, ExportZipError::OutputConflict { .. }));
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn removes_temporary_output_when_archive_exceeds_limit() {
        let root = temp_dir();
        let session_dir = root.join("session");
        fs::create_dir_all(&session_dir).await.unwrap();
        fs::write(session_dir.join("wire.jsonl"), b"content")
            .await
            .unwrap();
        let output = root.join("session.zip");

        let error = write_export_zip(WriteExportZipArgs {
            output_path: output.clone(),
            manifest: manifest(),
            session_dir: session_dir.clone(),
            session_files: vec![SessionZipEntry::Path(session_dir.join("wire.jsonl"))],
            extra_entries: Vec::new(),
            cancellation: None,
            max_archive_bytes: Some(1),
        })
        .await
        .unwrap_err();

        assert!(matches!(error, ExportZipError::TooLarge { .. }));
        assert!(!output.exists());
        let mut entries = fs::read_dir(&root).await.unwrap();
        assert!(entries.next_entry().await.unwrap().is_some());
        assert!(entries.next_entry().await.unwrap().is_none());
        fs::remove_dir_all(root).await.unwrap();
    }
}
