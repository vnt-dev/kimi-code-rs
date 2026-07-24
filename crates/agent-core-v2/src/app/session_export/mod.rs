//! Session diagnostic export contract and manifest.
//! Original: `packages/agent-core-v2/src/app/sessionExport`.
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
