//! Session diagnostic export contract and manifest.
//! Original: `packages/agent-core-v2/src/app/sessionExport`.
use std::{io, path::Path};

use tokio_util::sync::CancellationToken;

pub mod contract;
pub mod file_source;
pub mod manifest;
pub mod service;
pub mod wire_scan;
pub mod zip;
pub use contract::*;
pub use file_source::{ZipSource, ZipSourceIdentity, open_zip_source};
pub use manifest::*;
pub use service::{
    ExportSessionDirectoryArgs, ExportSessionDirectoryError, ExportSessionDirectoryResult,
    ExportSessionDirectorySummary, default_export_zip_name, export_session_directory,
};
pub use wire_scan::{normalize_timestamp_ms, scan_session_wire};
pub use zip::{
    ExportZipError, ExtraZipEntry, SessionZipEntry, WriteExportZipArgs, collect_files_recursive,
    write_export_zip,
};

/// Cancellation guard shared by every persisted-export stage. Returns an
/// `Interrupted` error as soon as the caller's token is cancelled.
pub(super) fn check(cancellation: Option<&CancellationToken>) -> io::Result<()> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "session export cancelled",
        ))
    } else {
        Ok(())
    }
}

/// The archive target path of an export entry, regardless of its source kind.
pub(super) fn session_entry_path(entry: &SessionZipEntry) -> &Path {
    match entry {
        SessionZipEntry::Path(path) | SessionZipEntry::Source { path, .. } => path,
    }
}

/// Filesystem identity used to detect when several entries share one source
/// file. Non-Unix platforms report no inode/device and therefore never match.
#[cfg(unix)]
pub(super) fn file_identity(metadata: &std::fs::Metadata) -> ZipSourceIdentity {
    use std::os::unix::fs::MetadataExt;
    ZipSourceIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

/// See the Unix variant; identity is always empty on other platforms.
#[cfg(not(unix))]
pub(super) fn file_identity(_: &std::fs::Metadata) -> ZipSourceIdentity {
    ZipSourceIdentity {
        device: 0,
        inode: 0,
    }
}
