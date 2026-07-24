//! Persisted session-directory export orchestration.
//! Original: `packages/agent-core-v2/src/app/sessionExport/sessionExportService.ts`,
//! `exportSessionDirectory()`, `defaultExportZipName()`, and
//! `openOptionalZipSource()`.
//!
//! The source's App-scoped `SessionExportService.export()` additionally
//! coordinates live session and log flushing. Those lifecycle services have
//! not yet been migrated; this module ports the self-contained persisted-file
//! export method used after that flushing has completed.
use std::{
    io,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use super::{
    BuildExportManifestArgs, ExportSessionManifest, ExportSessionPayload, ExportZipError,
    ExtraZipEntry, SessionZipEntry, ZipSource, build_export_manifest, collect_files_recursive,
    open_zip_source, scan_session_wire, write_export_zip,
};

const SESSION_LOG_REL: &str = "logs/kimi-code.log";
const GLOBAL_LOG_REL: &str = "logs/global/kimi-code.log";
const WEB_LOG_REL: &str = "logs/kimi-web.jsonl";

#[derive(Clone, Debug)]
pub struct ExportSessionDirectorySummary {
    pub id: String,
    pub title: Option<String>,
    pub workspace_dir: Option<String>,
    pub session_dir: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ExportSessionDirectoryArgs {
    pub request: ExportSessionPayload,
    pub summary: ExportSessionDirectorySummary,
    pub global_log_path: Option<PathBuf>,
    pub web_log: Option<String>,
    pub cancellation: Option<CancellationToken>,
    pub max_archive_bytes: Option<u64>,
    /// Injected by the future host-facing service. The source obtains this
    /// from `process.version` while building its manifest.
    pub nodejs_version: String,
    /// Injected to make export file naming and manifest timestamps testable.
    pub now: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportSessionDirectoryResult {
    pub zip_path: PathBuf,
    pub entries: Vec<String>,
    pub session_dir: PathBuf,
    pub manifest: ExportSessionManifest,
}

#[derive(Debug, Error)]
pub enum ExportSessionDirectoryError {
    #[error("session \"{session_id}\" has no exportable directory at \"{session_dir}\"")]
    NotFound {
        session_id: String,
        session_dir: PathBuf,
    },

    #[error(transparent)]
    Io(#[from] io::Error),

    #[error(transparent)]
    Zip(#[from] ExportZipError),
}

pub async fn export_session_directory(
    args: ExportSessionDirectoryArgs,
) -> Result<ExportSessionDirectoryResult, ExportSessionDirectoryError> {
    check(args.cancellation.as_ref())?;
    let session_dir = args.summary.session_dir.clone();
    let session_log_path = session_dir.join(SESSION_LOG_REL);
    let session_log_source =
        open_optional_zip_source(&session_log_path, args.cancellation.as_ref()).await?;
    let global_source = if args.request.include_global_log == Some(true) {
        match args.global_log_path.as_deref() {
            Some(path) => open_optional_zip_source(path, args.cancellation.as_ref()).await?,
            None => None,
        }
    } else {
        None
    };

    let session_files = collect_files_recursive(&session_dir).await?;
    if session_files.is_empty() && session_log_source.is_none() {
        return Err(ExportSessionDirectoryError::NotFound {
            session_id: args.summary.id,
            session_dir,
        });
    }

    let session_scan = scan_session_wire(&session_dir, args.cancellation.as_ref()).await?;
    let mut selected_session_files = session_files
        .into_iter()
        .filter(|file| file != &session_log_path)
        .map(SessionZipEntry::Path)
        .collect::<Vec<_>>();
    if let Some(source) = session_log_source {
        selected_session_files.push(SessionZipEntry::Source {
            path: session_log_path,
            source,
        });
    }
    selected_session_files
        .sort_by(|left, right| session_entry_path(left).cmp(session_entry_path(right)));

    let bundled_web_log = args.web_log.is_some();
    let base_manifest = build_export_manifest(BuildExportManifestArgs {
        summary: super::ExportSessionManifestSummary {
            id: args.summary.id.clone(),
            title: args.summary.title.clone(),
            workspace_dir: args.summary.workspace_dir.clone(),
        },
        now: args.now.clone(),
        version: args.request.version.clone(),
        wire_protocol_version: None,
        session_scan,
        session_log_path: selected_session_files
            .iter()
            .any(is_session_log_source)
            .then(|| SESSION_LOG_REL.to_owned()),
        global_log_path: None,
        web_log_path: bundled_web_log.then(|| WEB_LOG_REL.to_owned()),
        install_source: args.request.install_source.clone(),
        shell_env: args.request.shell_env.clone(),
        nodejs_version: args.nodejs_version,
    });

    let mut extra_entries = Vec::new();
    if let Some(web_log) = args.web_log {
        extra_entries.push(ExtraZipEntry::Data {
            data: web_log.into_bytes(),
            target: WEB_LOG_REL.to_owned(),
        });
    }
    let manifest = match global_source {
        Some(source) => {
            extra_entries.push(ExtraZipEntry::Source {
                source,
                target: GLOBAL_LOG_REL.to_owned(),
            });
            ExportSessionManifest {
                global_log_path: Some(GLOBAL_LOG_REL.to_owned()),
                ..base_manifest
            }
        }
        None => base_manifest,
    };
    let output_path = match args.request.output_path.as_deref() {
        Some(path) => std::path::absolute(path)?,
        None => std::path::absolute(default_export_zip_name(&args.summary.id, args.now))?,
    };
    let entries = write_export_zip(super::WriteExportZipArgs {
        output_path: output_path.clone(),
        manifest: manifest.clone(),
        session_dir: session_dir.clone(),
        session_files: selected_session_files,
        extra_entries,
        cancellation: args.cancellation,
        max_archive_bytes: args.max_archive_bytes,
    })
    .await?;

    Ok(ExportSessionDirectoryResult {
        zip_path: output_path,
        entries,
        session_dir,
        manifest,
    })
}

pub fn default_export_zip_name(session_id: &str, now: DateTime<Utc>) -> String {
    let short_id = session_id.get(..8).unwrap_or(session_id);
    format!("kimi-debug-{short_id}-{}.zip", now.format("%Y%m%d-%H%M%S"))
}

async fn open_optional_zip_source(
    path: &Path,
    cancellation: Option<&CancellationToken>,
) -> io::Result<Option<ZipSource>> {
    match open_zip_source(path, cancellation).await {
        Ok(source) => Ok(Some(source)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn session_entry_path(entry: &SessionZipEntry) -> &Path {
    match entry {
        SessionZipEntry::Path(path) | SessionZipEntry::Source { path, .. } => path,
    }
}

fn is_session_log_source(entry: &SessionZipEntry) -> bool {
    matches!(entry, SessionZipEntry::Source { path, .. } if path.ends_with(SESSION_LOG_REL))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_name_uses_source_timestamp_shape() {
        let now = DateTime::parse_from_rfc3339("2026-02-03T04:05:06.789Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            default_export_zip_name("123456789", now),
            "kimi-debug-12345678-20260203-040506.zip"
        );
    }

    #[tokio::test]
    async fn reports_missing_session_directory() {
        let root =
            std::env::temp_dir().join(format!("kimi-export-session-{}", uuid::Uuid::new_v4()));
        let missing = root.join("missing");
        let error = export_session_directory(ExportSessionDirectoryArgs {
            request: ExportSessionPayload {
                session_id: "session".to_owned(),
                output_path: Some(root.join("out.zip").to_string_lossy().into_owned()),
                include_global_log: None,
                version: "1.0.0".to_owned(),
                install_source: None,
                shell_env: None,
            },
            summary: ExportSessionDirectorySummary {
                id: "session".to_owned(),
                title: None,
                workspace_dir: None,
                session_dir: missing.clone(),
            },
            global_log_path: None,
            web_log: None,
            cancellation: None,
            max_archive_bytes: None,
            nodejs_version: "22.0.0".to_owned(),
            now: Utc::now(),
        })
        .await
        .unwrap_err();
        assert!(
            matches!(error, ExportSessionDirectoryError::NotFound { session_dir, .. } if session_dir == missing)
        );
    }
}
