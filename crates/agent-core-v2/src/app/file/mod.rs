//! Process-global uploaded-file storage.

pub mod contract;
pub mod service;

pub use contract::{
    DEFAULT_MAX_UPLOAD_BYTES, FILE_NOT_FOUND, FILE_SERVICE_ID, FILE_TOO_LARGE, FileByteStream,
    FileError, FileMeta, FileReadRange, FileReadStreamFactory, FileServiceContract,
    FileServiceError, FileServiceHandle, FileServiceResult, GetResult, SaveOptions,
    ensure_file_errors_registered, file_not_found_error, file_too_large_error, is_file_error,
};
pub use service::{FileService, register_file_service};
