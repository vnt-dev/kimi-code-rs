use std::{
    error::Error,
    fmt,
    future::Future,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use sha2::{Digest, Sha256};

use crate::{
    cli::sub::export::{ExportSessionInput, ExportSessionResult},
    oauth::managed_feedback_upload::{
        CompleteFeedbackUploadBody, CompleteFeedbackUploadPart, CreateFeedbackUploadUrlBody,
        FetchCompleteFeedbackUploadResult, FetchCreateFeedbackUploadUrlResult,
    },
    sdk::types::{SessionSummary, ShellEnvironment},
    tui::commands::prompts::FeedbackAttachmentLevel,
    utils::paths::{HomeDirectoryUnavailable, get_cache_dir, get_log_dir},
};

use super::{
    archive::{
        FeedbackArchive, FeedbackArchiveError, cleanup_feedback_archive,
        create_feedback_archive_path_in,
    },
    codebase::{
        FeedbackCodebaseScanResult, PackageCodebaseError, ScanCancellation, ScanCodebaseOptions,
        package_codebase, scan_codebase,
    },
    upload::{
        CompleteFeedbackUploadUrlInput, CreateFeedbackUploadUrlInput,
        CreateFeedbackUploadUrlResult, FeedbackUploadApiError, FeedbackUploadPart,
        FeedbackUploadUrlApi, PartUploadTransport, ReqwestPartUploadTransport, UploadArchiveError,
        UploadArchiveOptions, upload_archive_with_transport,
    },
};

pub const CODEBASE_ARCHIVE_FILENAME: &str = "repo.zip";
pub const SESSION_ARCHIVE_FILENAME: &str = "session.zip";
const CODEBASE_SCAN_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackAttachmentState {
    pub work_dir: PathBuf,
    pub session_id: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackAttachmentPaths {
    pub cache_dir: PathBuf,
    pub log_dir: PathBuf,
}

impl FeedbackAttachmentPaths {
    pub fn system() -> Result<Self, HomeDirectoryUnavailable> {
        Ok(Self {
            cache_dir: get_cache_dir()?,
            log_dir: get_log_dir()?,
        })
    }
}

#[derive(Debug)]
pub struct FeedbackAttachmentHostError(Box<dyn Error + Send + Sync>);

impl FeedbackAttachmentHostError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for FeedbackAttachmentHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for FeedbackAttachmentHostError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[async_trait]
pub trait FeedbackAttachmentHost: Send + Sync {
    fn attachment_state(&self) -> FeedbackAttachmentState;

    async fn list_sessions(
        &self,
        work_dir: &Path,
    ) -> Result<Vec<SessionSummary>, FeedbackAttachmentHostError>;

    async fn export_session(
        &self,
        input: ExportSessionInput,
    ) -> Result<ExportSessionResult, FeedbackAttachmentHostError>;

    async fn install_source(&self) -> Result<String, FeedbackAttachmentHostError>;

    fn shell_environment(&self) -> ShellEnvironment;

    async fn create_feedback_upload_url(
        &self,
        input: CreateFeedbackUploadUrlBody,
    ) -> FetchCreateFeedbackUploadUrlResult;

    async fn complete_feedback_upload(
        &self,
        input: CompleteFeedbackUploadBody,
    ) -> FetchCompleteFeedbackUploadResult;
}

#[derive(Debug)]
pub enum FeedbackAttachmentError {
    Home(HomeDirectoryUnavailable),
    Archive(FeedbackArchiveError),
}

impl fmt::Display for FeedbackAttachmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Home(error) => error.fmt(formatter),
            Self::Archive(error) => error.fmt(formatter),
        }
    }
}

impl Error for FeedbackAttachmentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Home(error) => Some(error),
            Self::Archive(error) => Some(error),
        }
    }
}

impl From<FeedbackArchiveError> for FeedbackAttachmentError {
    fn from(error: FeedbackArchiveError) -> Self {
        Self::Archive(error)
    }
}

#[derive(Debug)]
enum ProduceArchiveError {
    Host(FeedbackAttachmentHostError),
    Io(std::io::Error),
    Package(PackageCodebaseError),
}

impl fmt::Display for ProduceArchiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Host(error) => error.fmt(formatter),
            Self::Io(error) => error.fmt(formatter),
            Self::Package(error) => error.fmt(formatter),
        }
    }
}

impl Error for ProduceArchiveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Host(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Package(error) => Some(error),
        }
    }
}

impl From<FeedbackAttachmentHostError> for ProduceArchiveError {
    fn from(error: FeedbackAttachmentHostError) -> Self {
        Self::Host(error)
    }
}

impl From<std::io::Error> for ProduceArchiveError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<PackageCodebaseError> for ProduceArchiveError {
    fn from(error: PackageCodebaseError) -> Self {
        Self::Package(error)
    }
}

struct HostFeedbackUploadApi<'a> {
    host: &'a dyn FeedbackAttachmentHost,
}

#[derive(Debug)]
struct AttachmentMessageError(String);

impl fmt::Display for AttachmentMessageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for AttachmentMessageError {}

#[async_trait]
impl FeedbackUploadUrlApi for HostFeedbackUploadApi<'_> {
    // Original: `feedback-attachments.ts`, `createFeedbackUploadApi().createUploadUrl()`.
    async fn create_upload_url(
        &self,
        input: CreateFeedbackUploadUrlInput,
    ) -> Result<CreateFeedbackUploadUrlResult, FeedbackUploadApiError> {
        let result = self
            .host
            .create_feedback_upload_url(CreateFeedbackUploadUrlBody {
                file_hash: input.sha256,
                file_name: input.filename,
                file_size: input.size.try_into().map_err(|_| {
                    FeedbackUploadApiError::new(AttachmentMessageError(
                        "feedback archive size is outside the backend integer range".to_owned(),
                    ))
                })?,
                feedback_id: input.feedback_id,
            })
            .await;
        match result {
            FetchCreateFeedbackUploadUrlResult::Ok { upload_id, parts } => {
                let parts = parts
                    .into_iter()
                    .map(|part| {
                        let size = u64::try_from(part.size).map_err(|_| {
                            FeedbackUploadApiError::new(AttachmentMessageError(format!(
                                "feedback upload part {} has a negative size",
                                part.part_number
                            )))
                        })?;
                        Ok(FeedbackUploadPart {
                            part_number: part.part_number,
                            url: part.url,
                            method: part.method,
                            size,
                        })
                    })
                    .collect::<Result<Vec<_>, FeedbackUploadApiError>>()?;
                Ok(CreateFeedbackUploadUrlResult { upload_id, parts })
            }
            FetchCreateFeedbackUploadUrlResult::Error { message, .. } => {
                Err(FeedbackUploadApiError::new(AttachmentMessageError(message)))
            }
        }
    }

    // Original: `createFeedbackUploadApi().completeUpload()`.
    async fn complete_upload(
        &self,
        input: CompleteFeedbackUploadUrlInput,
    ) -> Result<(), FeedbackUploadApiError> {
        match self
            .host
            .complete_feedback_upload(CompleteFeedbackUploadBody {
                upload_id: input.upload_id,
                parts: input
                    .parts
                    .into_iter()
                    .map(|part| CompleteFeedbackUploadPart {
                        part_number: part.part_number,
                        etag: part.etag,
                    })
                    .collect(),
            })
            .await
        {
            FetchCompleteFeedbackUploadResult::Ok => Ok(()),
            FetchCompleteFeedbackUploadResult::Error { message, .. } => {
                Err(FeedbackUploadApiError::new(AttachmentMessageError(message)))
            }
        }
    }
}

// Original: `src/feedback/feedback-attachments.ts`,
// `submitFeedbackWithAttachments()`.
pub async fn submit_feedback_with_attachments(
    host: &dyn FeedbackAttachmentHost,
    feedback_id: i64,
    level: FeedbackAttachmentLevel,
) -> Result<bool, FeedbackAttachmentError> {
    if level == FeedbackAttachmentLevel::None {
        return Ok(false);
    }
    let paths = FeedbackAttachmentPaths::system().map_err(FeedbackAttachmentError::Home)?;
    submit_feedback_with_attachments_in(
        host,
        &ReqwestPartUploadTransport::default(),
        feedback_id,
        level,
        &paths,
    )
    .await
}

pub async fn submit_feedback_with_attachments_in(
    host: &dyn FeedbackAttachmentHost,
    transport: &dyn PartUploadTransport,
    feedback_id: i64,
    level: FeedbackAttachmentLevel,
    paths: &FeedbackAttachmentPaths,
) -> Result<bool, FeedbackAttachmentError> {
    let api = HostFeedbackUploadApi { host };
    match level {
        FeedbackAttachmentLevel::None => Ok(false),
        FeedbackAttachmentLevel::Logs => {
            let uploaded =
                prepare_and_upload_session_archive(host, &api, transport, feedback_id, None, paths)
                    .await?;
            Ok(!uploaded)
        }
        FeedbackAttachmentLevel::LogsAndCodebase => {
            let state = host.attachment_state();
            let (session_dir, scan) = tokio::join!(
                resolve_current_session_dir(host, &state),
                scan_codebase_for_feedback(&state.work_dir, &paths.log_dir),
            );
            let (uploaded_session, uploaded_codebase) = tokio::join!(
                prepare_and_upload_session_archive(
                    host,
                    &api,
                    transport,
                    feedback_id,
                    session_dir,
                    paths,
                ),
                prepare_and_upload_codebase_archive(&api, transport, feedback_id, scan, paths,),
            );
            Ok(!uploaded_session? || !uploaded_codebase?)
        }
    }
}

// Original: `prepareAndUploadSessionArchive()`.
async fn prepare_and_upload_session_archive(
    host: &dyn FeedbackAttachmentHost,
    api: &dyn FeedbackUploadUrlApi,
    transport: &dyn PartUploadTransport,
    feedback_id: i64,
    known_session_dir: Option<PathBuf>,
    paths: &FeedbackAttachmentPaths,
) -> Result<bool, FeedbackAttachmentError> {
    let state = host.attachment_state();
    let session_dir = match known_session_dir {
        Some(directory) => Some(directory),
        None => resolve_current_session_dir(host, &state).await,
    };
    if session_dir.is_none() {
        log_feedback_upload_error(
            &paths.log_dir,
            &AttachmentMessageError("cannot locate the current session directory".to_owned()),
        )
        .await;
        return Ok(false);
    }
    upload_produced_archive(
        api,
        transport,
        feedback_id,
        SESSION_ARCHIVE_FILENAME,
        paths,
        |archive_path| async move {
            let exported = host
                .export_session(ExportSessionInput {
                    id: state.session_id,
                    version: state.version,
                    install_source: host.install_source().await?,
                    shell_env: host.shell_environment(),
                    output_path: Some(archive_path.to_string_lossy().into_owned()),
                    include_global_log: Some(true),
                })
                .await?;
            archive_from_exported_session(Path::new(&exported.zip_path)).await
        },
    )
    .await
}

// Original: `prepareAndUploadCodebaseArchive()`.
async fn prepare_and_upload_codebase_archive(
    api: &dyn FeedbackUploadUrlApi,
    transport: &dyn PartUploadTransport,
    feedback_id: i64,
    scan: Option<FeedbackCodebaseScanResult>,
    paths: &FeedbackAttachmentPaths,
) -> Result<bool, FeedbackAttachmentError> {
    let Some(scan) = scan else {
        return Ok(false);
    };
    upload_produced_archive(
        api,
        transport,
        feedback_id,
        CODEBASE_ARCHIVE_FILENAME,
        paths,
        |archive_path| async move {
            package_codebase(&scan, &archive_path)
                .await
                .map_err(ProduceArchiveError::from)
        },
    )
    .await
}

// Original: `uploadProducedArchive()`.
async fn upload_produced_archive<F, Fut>(
    api: &dyn FeedbackUploadUrlApi,
    transport: &dyn PartUploadTransport,
    feedback_id: i64,
    filename: &str,
    paths: &FeedbackAttachmentPaths,
    produce: F,
) -> Result<bool, FeedbackAttachmentError>
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: Future<Output = Result<FeedbackArchive, ProduceArchiveError>>,
{
    let archive_path = create_feedback_archive_path_in(&paths.cache_dir, filename).await?;
    let result = async {
        let mut archive = produce(archive_path.archive_path.clone()).await?;
        archive.cleanup_dir = Some(archive_path.cleanup_dir.clone());
        upload_archive_with_transport(
            api,
            transport,
            &archive,
            feedback_id,
            UploadArchiveOptions::new(filename),
        )
        .await
        .map_err(ProducedUploadError::Upload)?;
        Ok::<(), ProducedUploadError>(())
    }
    .await;
    cleanup_feedback_archive(&archive_path.cleanup_dir).await;
    match result {
        Ok(()) => Ok(true),
        Err(error) => {
            log_feedback_upload_error(&paths.log_dir, &error).await;
            Ok(false)
        }
    }
}

#[derive(Debug)]
enum ProducedUploadError {
    Produce(ProduceArchiveError),
    Upload(UploadArchiveError),
}

impl From<ProduceArchiveError> for ProducedUploadError {
    fn from(error: ProduceArchiveError) -> Self {
        Self::Produce(error)
    }
}

impl fmt::Display for ProducedUploadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Produce(error) => error.fmt(formatter),
            Self::Upload(error) => error.fmt(formatter),
        }
    }
}

impl Error for ProducedUploadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Produce(error) => Some(error),
            Self::Upload(error) => Some(error),
        }
    }
}

// Original: `archiveFromExportedSession()`.
async fn archive_from_exported_session(
    zip_path: &Path,
) -> Result<FeedbackArchive, ProduceArchiveError> {
    let data = tokio::fs::read(zip_path).await?;
    let size = tokio::fs::metadata(zip_path).await?.len();
    let digest = Sha256::digest(&data);
    let hash = encode_hex(&digest);
    Ok(FeedbackArchive {
        path: zip_path.to_path_buf(),
        size,
        sha256: hash.clone(),
        fingerprint: hash,
        file_count: 1,
        cleanup_dir: None,
    })
}

// Original: `resolveCurrentSessionDir()`.
async fn resolve_current_session_dir(
    host: &dyn FeedbackAttachmentHost,
    state: &FeedbackAttachmentState,
) -> Option<PathBuf> {
    host.list_sessions(&state.work_dir)
        .await
        .ok()?
        .into_iter()
        .find(|session| session.id == state.session_id)
        .map(|session| PathBuf::from(session.session_dir))
}

// Original: `scanCodebaseForFeedback()`.
async fn scan_codebase_for_feedback(
    work_dir: &Path,
    log_dir: &Path,
) -> Option<FeedbackCodebaseScanResult> {
    let cancellation = ScanCancellation::default();
    let scan = scan_codebase(
        work_dir,
        ScanCodebaseOptions {
            cancellation: Some(cancellation.clone()),
            ..ScanCodebaseOptions::default()
        },
    );
    tokio::pin!(scan);
    let result = tokio::select! {
        result = &mut scan => result,
        () = tokio::time::sleep(CODEBASE_SCAN_TIMEOUT) => {
            cancellation.abort();
            scan.await
        }
    };
    match result {
        Ok(scan) => Some(scan),
        Err(error) => {
            log_feedback_upload_error(log_dir, &error).await;
            None
        }
    }
}

// Original: `logFeedbackUploadError()`.
async fn log_feedback_upload_error(log_dir: &Path, error: &(dyn Error + Send + Sync)) {
    let result = async {
        tokio::fs::create_dir_all(log_dir).await?;
        let timestamp =
            DateTime::<Utc>::from(SystemTime::now()).to_rfc3339_opts(SecondsFormat::Millis, true);
        let line = format!("{timestamp} {error}\n");
        let mut options = tokio::fs::OpenOptions::new();
        options.create(true).append(true);
        let mut file = options.open(log_dir.join("feedback-upload.log")).await?;
        tokio::io::AsyncWriteExt::write_all(&mut file, line.as_bytes()).await
    }
    .await;
    let _ = result;
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
    use std::sync::Mutex;

    use uuid::Uuid;

    use super::*;
    use crate::feedback::upload::CompletedUploadPart;
    use crate::oauth::managed_feedback_upload::FeedbackUploadPart as ManagedUploadPart;

    #[derive(Debug, Clone, Copy)]
    struct TestError(&'static str);

    impl fmt::Display for TestError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.0)
        }
    }

    impl Error for TestError {}

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("feedback-attachments-{}", Uuid::new_v4()));
            std::fs::create_dir_all(path.join("work")).expect("work dir");
            Self(path)
        }

        fn paths(&self) -> FeedbackAttachmentPaths {
            FeedbackAttachmentPaths {
                cache_dir: self.0.join("cache"),
                log_dir: self.0.join("logs"),
            }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct HostMock {
        state: FeedbackAttachmentState,
        session_available: bool,
        export_error: bool,
        export_inputs: Mutex<Vec<ExportSessionInput>>,
        create_inputs: Mutex<Vec<CreateFeedbackUploadUrlBody>>,
        complete_inputs: Mutex<Vec<CompleteFeedbackUploadBody>>,
    }

    impl HostMock {
        fn new(temp: &TempDir) -> Self {
            Self {
                state: FeedbackAttachmentState {
                    work_dir: temp.0.join("work"),
                    session_id: "ses-1".to_owned(),
                    version: "1.2.3".to_owned(),
                },
                session_available: true,
                export_error: false,
                export_inputs: Mutex::new(Vec::new()),
                create_inputs: Mutex::new(Vec::new()),
                complete_inputs: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl FeedbackAttachmentHost for HostMock {
        fn attachment_state(&self) -> FeedbackAttachmentState {
            self.state.clone()
        }

        async fn list_sessions(
            &self,
            work_dir: &Path,
        ) -> Result<Vec<SessionSummary>, FeedbackAttachmentHostError> {
            if !self.session_available {
                return Ok(Vec::new());
            }
            Ok(vec![SessionSummary {
                id: self.state.session_id.clone(),
                title: None,
                last_prompt: None,
                work_dir: work_dir.to_string_lossy().into_owned(),
                session_dir: work_dir.join(".session").to_string_lossy().into_owned(),
                created_at: None,
                updated_at: None,
                archived: None,
                metadata: None,
                additional_dirs: None,
            }])
        }

        async fn export_session(
            &self,
            input: ExportSessionInput,
        ) -> Result<ExportSessionResult, FeedbackAttachmentHostError> {
            self.export_inputs
                .lock()
                .expect("export inputs")
                .push(input.clone());
            if self.export_error {
                return Err(FeedbackAttachmentHostError::new(TestError("export failed")));
            }
            let path = PathBuf::from(input.output_path.expect("output path"));
            tokio::fs::write(&path, b"session zip")
                .await
                .map_err(FeedbackAttachmentHostError::new)?;
            Ok(ExportSessionResult {
                zip_path: path.to_string_lossy().into_owned(),
            })
        }

        async fn install_source(&self) -> Result<String, FeedbackAttachmentHostError> {
            Ok("npm-global".to_owned())
        }

        fn shell_environment(&self) -> ShellEnvironment {
            ShellEnvironment {
                term: Some("xterm".to_owned()),
                ..ShellEnvironment::default()
            }
        }

        async fn create_feedback_upload_url(
            &self,
            input: CreateFeedbackUploadUrlBody,
        ) -> FetchCreateFeedbackUploadUrlResult {
            self.create_inputs
                .lock()
                .expect("create inputs")
                .push(input.clone());
            FetchCreateFeedbackUploadUrlResult::Ok {
                upload_id: self.create_inputs.lock().expect("create inputs").len() as i64,
                parts: vec![ManagedUploadPart {
                    part_number: 1,
                    url: "https://example.test/upload".to_owned(),
                    method: "PUT".to_owned(),
                    size: input.file_size,
                }],
            }
        }

        async fn complete_feedback_upload(
            &self,
            input: CompleteFeedbackUploadBody,
        ) -> FetchCompleteFeedbackUploadResult {
            self.complete_inputs
                .lock()
                .expect("complete inputs")
                .push(input);
            FetchCompleteFeedbackUploadResult::Ok
        }
    }

    #[derive(Default)]
    struct TransportMock {
        requests: Mutex<Vec<super::super::upload::PartUploadRequest>>,
        fail_filename: Option<String>,
    }

    #[async_trait]
    impl PartUploadTransport for TransportMock {
        async fn upload_part(
            &self,
            request: super::super::upload::PartUploadRequest,
        ) -> Result<CompletedUploadPart, super::super::upload::UploadPartError> {
            self.requests
                .lock()
                .expect("requests")
                .push(request.clone());
            if self.fail_filename.as_ref().is_some_and(|filename| {
                request.file_path.file_name().and_then(|name| name.to_str()) == Some(filename)
            }) {
                return Err(super::super::upload::UploadPartError::Http {
                    part_number: request.part.part_number,
                    status: 400,
                    response_body: "rejected".to_owned(),
                });
            }
            Ok(CompletedUploadPart {
                part_number: request.part.part_number,
                etag: "\"etag\"".to_owned(),
            })
        }
    }

    #[tokio::test]
    async fn no_attachment_skips_paths_sessions_and_uploads() {
        let temp = TempDir::new();
        let host = HostMock::new(&temp);
        let partial = submit_feedback_with_attachments_in(
            &host,
            &TransportMock::default(),
            3,
            FeedbackAttachmentLevel::None,
            &temp.paths(),
        )
        .await
        .expect("none");
        assert!(!partial);
        assert!(host.export_inputs.lock().expect("exports").is_empty());
        assert!(host.create_inputs.lock().expect("creates").is_empty());
    }

    #[tokio::test]
    async fn logs_export_upload_and_cleanup_share_the_expected_context() {
        let temp = TempDir::new();
        let host = HostMock::new(&temp);
        let transport = TransportMock::default();
        let partial = submit_feedback_with_attachments_in(
            &host,
            &transport,
            3,
            FeedbackAttachmentLevel::Logs,
            &temp.paths(),
        )
        .await
        .expect("logs");

        assert!(!partial);
        let exports = host.export_inputs.lock().expect("exports");
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].id, "ses-1");
        assert_eq!(exports[0].version, "1.2.3");
        assert_eq!(exports[0].install_source, "npm-global");
        assert_eq!(exports[0].include_global_log, Some(true));
        let requests = transport.requests.lock().expect("requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0]
                .file_path
                .file_name()
                .and_then(|name| name.to_str()),
            Some(SESSION_ARCHIVE_FILENAME)
        );
        assert!(!requests[0].file_path.exists());
        assert_eq!(host.complete_inputs.lock().expect("complete").len(), 1);
    }

    #[tokio::test]
    async fn logs_and_codebase_prepare_and_upload_both_archives() {
        let temp = TempDir::new();
        std::fs::write(temp.0.join("work/main.rs"), b"fn main() {}\n").expect("source");
        let host = HostMock::new(&temp);
        let transport = TransportMock::default();

        let partial = submit_feedback_with_attachments_in(
            &host,
            &transport,
            7,
            FeedbackAttachmentLevel::LogsAndCodebase,
            &temp.paths(),
        )
        .await
        .expect("attachments");

        assert!(!partial);
        let mut filenames = transport
            .requests
            .lock()
            .expect("requests")
            .iter()
            .filter_map(|request| {
                request
                    .file_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
        filenames.sort();
        assert_eq!(filenames, ["repo.zip", "session.zip"]);
        assert_eq!(host.complete_inputs.lock().expect("complete").len(), 2);
    }

    #[tokio::test]
    async fn scan_failure_still_uploads_session_and_reports_partial_failure() {
        let temp = TempDir::new();
        let mut host = HostMock::new(&temp);
        host.state.work_dir = temp.0.join("missing-work-dir");
        let transport = TransportMock::default();

        let partial = submit_feedback_with_attachments_in(
            &host,
            &transport,
            3,
            FeedbackAttachmentLevel::LogsAndCodebase,
            &temp.paths(),
        )
        .await
        .expect("partial");

        assert!(partial);
        assert_eq!(transport.requests.lock().expect("requests").len(), 1);
        let log =
            std::fs::read_to_string(temp.0.join("logs/feedback-upload.log")).expect("feedback log");
        assert!(!log.is_empty());
    }

    #[tokio::test]
    async fn failed_upload_is_nonfatal_logged_and_always_cleans_archive_directory() {
        let temp = TempDir::new();
        let host = HostMock::new(&temp);
        let transport = TransportMock {
            requests: Mutex::new(Vec::new()),
            fail_filename: Some(SESSION_ARCHIVE_FILENAME.to_owned()),
        };

        let partial = submit_feedback_with_attachments_in(
            &host,
            &transport,
            3,
            FeedbackAttachmentLevel::Logs,
            &temp.paths(),
        )
        .await
        .expect("partial");

        assert!(partial);
        let path = transport.requests.lock().expect("requests")[0]
            .file_path
            .clone();
        assert!(!path.exists());
        assert!(
            std::fs::read_to_string(temp.0.join("logs/feedback-upload.log"))
                .expect("feedback log")
                .contains("HTTP 400 rejected")
        );
        assert!(host.complete_inputs.lock().expect("complete").is_empty());
    }
}
