use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use super::archive::FeedbackArchive;

const MAX_ARCHIVE_SIZE: u64 = 524_288_000;
const DEFAULT_CONCURRENCY: usize = 3;
const DEFAULT_MAX_RETRIES: u32 = 3;
const DEFAULT_PART_TIMEOUT: Duration = Duration::from_secs(60);
const RETRY_BASE_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackUploadPart {
    pub part_number: i64,
    pub url: String,
    pub method: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateFeedbackUploadUrlInput {
    pub feedback_id: i64,
    pub filename: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateFeedbackUploadUrlResult {
    pub upload_id: i64,
    pub parts: Vec<FeedbackUploadPart>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedUploadPart {
    pub part_number: i64,
    pub etag: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteFeedbackUploadUrlInput {
    pub upload_id: i64,
    pub parts: Vec<CompletedUploadPart>,
}

#[derive(Debug)]
pub struct FeedbackUploadApiError(Box<dyn Error + Send + Sync>);

impl FeedbackUploadApiError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for FeedbackUploadApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for FeedbackUploadApiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[async_trait]
pub trait FeedbackUploadUrlApi: Send + Sync {
    async fn create_upload_url(
        &self,
        input: CreateFeedbackUploadUrlInput,
    ) -> Result<CreateFeedbackUploadUrlResult, FeedbackUploadApiError>;

    async fn complete_upload(
        &self,
        input: CompleteFeedbackUploadUrlInput,
    ) -> Result<(), FeedbackUploadApiError>;
}

pub type UploadProgressCallback = Arc<dyn Fn(u64, u64) + Send + Sync>;

#[derive(Clone)]
pub struct UploadArchiveOptions {
    pub filename: String,
    pub timeout: Duration,
    pub concurrency: usize,
    pub max_retries: u32,
    pub on_progress: Option<UploadProgressCallback>,
}

impl fmt::Debug for UploadArchiveOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UploadArchiveOptions")
            .field("filename", &self.filename)
            .field("timeout", &self.timeout)
            .field("concurrency", &self.concurrency)
            .field("max_retries", &self.max_retries)
            .field(
                "on_progress",
                &self.on_progress.as_ref().map(|_| "callback"),
            )
            .finish()
    }
}

impl UploadArchiveOptions {
    pub fn new(filename: impl Into<String>) -> Self {
        Self {
            filename: filename.into(),
            timeout: DEFAULT_PART_TIMEOUT,
            concurrency: DEFAULT_CONCURRENCY,
            max_retries: DEFAULT_MAX_RETRIES,
            on_progress: None,
        }
    }
}

#[derive(Debug)]
pub enum UploadArchiveError {
    ArchiveTooLarge { size: u64, maximum: u64 },
    Api(FeedbackUploadApiError),
    Part(UploadPartError),
}

impl fmt::Display for UploadArchiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ArchiveTooLarge { size, maximum } => write!(
                formatter,
                "Failed to upload archive: size {size} exceeds maximum allowed size {maximum}."
            ),
            Self::Api(error) => error.fmt(formatter),
            Self::Part(error) => error.fmt(formatter),
        }
    }
}

impl Error for UploadArchiveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Api(error) => Some(error),
            Self::Part(error) => Some(error),
            Self::ArchiveTooLarge { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum UploadPartError {
    Http {
        part_number: i64,
        status: u16,
        response_body: String,
    },
    TimedOut {
        part_number: i64,
    },
    MissingEtag {
        part_number: i64,
    },
    InvalidMethod {
        part_number: i64,
        message: String,
    },
    Io {
        part_number: i64,
        source: std::io::Error,
    },
    Request {
        part_number: i64,
        source: reqwest::Error,
    },
}

impl UploadPartError {
    fn is_retryable(&self) -> bool {
        match self {
            Self::Http { status, .. } => *status >= 500 || *status == 408 || *status == 429,
            Self::TimedOut { .. }
            | Self::MissingEtag { .. }
            | Self::InvalidMethod { .. }
            | Self::Io { .. }
            | Self::Request { .. } => true,
        }
    }
}

impl fmt::Display for UploadPartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http {
                part_number,
                status,
                response_body,
            } => {
                write!(
                    formatter,
                    "Failed to upload part {part_number}: HTTP {status}"
                )?;
                if !response_body.is_empty() {
                    write!(formatter, " {response_body}")?;
                }
                Ok(())
            }
            Self::TimedOut { part_number } => {
                write!(
                    formatter,
                    "Failed to upload part {part_number}: upload timed out."
                )
            }
            Self::MissingEtag { part_number } => write!(
                formatter,
                "Failed to upload part {part_number}: missing ETag in response."
            ),
            Self::InvalidMethod {
                part_number,
                message,
            } => write!(
                formatter,
                "Failed to upload part {part_number}: invalid HTTP method: {message}"
            ),
            Self::Io {
                part_number,
                source,
            } => write!(formatter, "Failed to upload part {part_number}: {source}"),
            Self::Request {
                part_number,
                source,
            } => write!(formatter, "Failed to upload part {part_number}: {source}"),
        }
    }
}

impl Error for UploadPartError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Request { source, .. } => Some(source),
            Self::Http { .. }
            | Self::TimedOut { .. }
            | Self::MissingEtag { .. }
            | Self::InvalidMethod { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PartUploadRequest {
    pub file_path: PathBuf,
    pub part: FeedbackUploadPart,
    pub start: u64,
    pub timeout: Duration,
}

#[async_trait]
pub trait PartUploadTransport: Send + Sync {
    async fn upload_part(
        &self,
        request: PartUploadRequest,
    ) -> Result<CompletedUploadPart, UploadPartError>;
}

#[derive(Debug, Clone, Default)]
pub struct ReqwestPartUploadTransport {
    client: reqwest::Client,
}

#[async_trait]
impl PartUploadTransport for ReqwestPartUploadTransport {
    async fn upload_part(
        &self,
        request: PartUploadRequest,
    ) -> Result<CompletedUploadPart, UploadPartError> {
        let part_number = request.part.part_number;
        let operation = async {
            let method =
                reqwest::Method::from_bytes(request.part.method.as_bytes()).map_err(|error| {
                    UploadPartError::InvalidMethod {
                        part_number,
                        message: error.to_string(),
                    }
                })?;
            let mut file = tokio::fs::File::open(&request.file_path)
                .await
                .map_err(|source| UploadPartError::Io {
                    part_number,
                    source,
                })?;
            file.seek(std::io::SeekFrom::Start(request.start))
                .await
                .map_err(|source| UploadPartError::Io {
                    part_number,
                    source,
                })?;
            let stream = ReaderStream::new(file.take(request.part.size));
            let response = self
                .client
                .request(method, &request.part.url)
                .header(reqwest::header::CONTENT_LENGTH, request.part.size)
                .body(reqwest::Body::wrap_stream(stream))
                .send()
                .await
                .map_err(|source| UploadPartError::Request {
                    part_number,
                    source,
                })?;
            let status = response.status();
            if !status.is_success() {
                let response_body = response.text().await.unwrap_or_default();
                return Err(UploadPartError::Http {
                    part_number,
                    status: status.as_u16(),
                    response_body,
                });
            }
            let etag = response
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|value| value.to_str().ok())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or(UploadPartError::MissingEtag { part_number })?;
            Ok(CompletedUploadPart { part_number, etag })
        };
        match tokio::time::timeout(request.timeout, operation).await {
            Ok(result) => result,
            Err(_) => Err(UploadPartError::TimedOut { part_number }),
        }
    }
}

#[derive(Debug, Clone)]
struct PartLayout {
    part: FeedbackUploadPart,
    start: u64,
}

// Original: `src/feedback/upload.ts`, `uploadArchive()`.
pub async fn upload_archive(
    api: &dyn FeedbackUploadUrlApi,
    archive: &FeedbackArchive,
    feedback_id: i64,
    options: UploadArchiveOptions,
) -> Result<(), UploadArchiveError> {
    upload_archive_with_transport(
        api,
        &ReqwestPartUploadTransport::default(),
        archive,
        feedback_id,
        options,
    )
    .await
}

pub async fn upload_archive_with_transport(
    api: &dyn FeedbackUploadUrlApi,
    transport: &dyn PartUploadTransport,
    archive: &FeedbackArchive,
    feedback_id: i64,
    options: UploadArchiveOptions,
) -> Result<(), UploadArchiveError> {
    if archive.size > MAX_ARCHIVE_SIZE {
        return Err(UploadArchiveError::ArchiveTooLarge {
            size: archive.size,
            maximum: MAX_ARCHIVE_SIZE,
        });
    }
    let created = api
        .create_upload_url(CreateFeedbackUploadUrlInput {
            feedback_id,
            filename: options.filename.clone(),
            size: archive.size,
            sha256: archive.sha256.clone(),
        })
        .await
        .map_err(UploadArchiveError::Api)?;
    let completed = upload_parts(
        transport,
        &archive.path,
        created.parts,
        archive.size,
        &options,
    )
    .await
    .map_err(UploadArchiveError::Part)?;
    api.complete_upload(CompleteFeedbackUploadUrlInput {
        upload_id: created.upload_id,
        parts: completed,
    })
    .await
    .map_err(UploadArchiveError::Api)
}

fn layout_parts(mut parts: Vec<FeedbackUploadPart>) -> Vec<PartLayout> {
    parts.sort_by_key(|part| part.part_number);
    let mut offset = 0_u64;
    parts
        .into_iter()
        .map(|part| {
            let start = offset;
            offset = offset.saturating_add(part.size);
            PartLayout { part, start }
        })
        .collect()
}

async fn upload_parts(
    transport: &dyn PartUploadTransport,
    file_path: &Path,
    parts: Vec<FeedbackUploadPart>,
    total_bytes: u64,
    options: &UploadArchiveOptions,
) -> Result<Vec<CompletedUploadPart>, UploadPartError> {
    let layout = layout_parts(parts);
    let result_count = layout.len();
    let concurrency = options.concurrency.max(1).min(result_count.max(1));
    let uploads = stream::iter(layout.into_iter().enumerate().map(|(index, entry)| {
        let file_path = file_path.to_path_buf();
        async move {
            let size = entry.part.size;
            let completed = upload_one_part_with_retry(transport, file_path, entry, options).await;
            (index, size, completed)
        }
    }))
    .buffer_unordered(concurrency);
    futures_util::pin_mut!(uploads);

    let mut results = vec![None; result_count];
    let mut uploaded_bytes = 0_u64;
    let mut first_error = None;
    while let Some((index, size, result)) = uploads.next().await {
        match result {
            Ok(completed) => {
                results[index] = Some(completed);
                uploaded_bytes = uploaded_bytes.saturating_add(size);
                if let Some(on_progress) = &options.on_progress {
                    on_progress(uploaded_bytes, total_bytes);
                }
            }
            Err(error) if first_error.is_none() => first_error = Some(error),
            Err(_) => {}
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    Ok(results.into_iter().flatten().collect())
}

async fn upload_one_part_with_retry(
    transport: &dyn PartUploadTransport,
    file_path: PathBuf,
    layout: PartLayout,
    options: &UploadArchiveOptions,
) -> Result<CompletedUploadPart, UploadPartError> {
    let mut attempt = 0_u32;
    loop {
        let result = transport
            .upload_part(PartUploadRequest {
                file_path: file_path.clone(),
                part: layout.part.clone(),
                start: layout.start,
                timeout: options.timeout,
            })
            .await;
        match result {
            Ok(completed) => return Ok(completed),
            Err(error) if attempt < options.max_retries && error.is_retryable() => {
                let multiplier = 1_u32.checked_shl(attempt).unwrap_or(u32::MAX);
                tokio::time::sleep(RETRY_BASE_DELAY.saturating_mul(multiplier)).await;
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::Mutex,
    };

    use super::*;

    #[derive(Debug, Clone, Copy)]
    struct TestError(&'static str);

    impl fmt::Display for TestError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.0)
        }
    }

    impl Error for TestError {}

    #[derive(Default)]
    struct ApiMock {
        created: Mutex<Vec<CreateFeedbackUploadUrlInput>>,
        completed: Mutex<Vec<CompleteFeedbackUploadUrlInput>>,
        parts: Vec<FeedbackUploadPart>,
    }

    #[async_trait]
    impl FeedbackUploadUrlApi for ApiMock {
        async fn create_upload_url(
            &self,
            input: CreateFeedbackUploadUrlInput,
        ) -> Result<CreateFeedbackUploadUrlResult, FeedbackUploadApiError> {
            self.created.lock().expect("created").push(input);
            Ok(CreateFeedbackUploadUrlResult {
                upload_id: 28,
                parts: self.parts.clone(),
            })
        }

        async fn complete_upload(
            &self,
            input: CompleteFeedbackUploadUrlInput,
        ) -> Result<(), FeedbackUploadApiError> {
            self.completed.lock().expect("completed").push(input);
            Ok(())
        }
    }

    struct TransportMock {
        requests: Mutex<Vec<PartUploadRequest>>,
        failures_left: Mutex<usize>,
        status: u16,
    }

    #[async_trait]
    impl PartUploadTransport for TransportMock {
        async fn upload_part(
            &self,
            request: PartUploadRequest,
        ) -> Result<CompletedUploadPart, UploadPartError> {
            self.requests
                .lock()
                .expect("requests")
                .push(request.clone());
            let mut failures = self.failures_left.lock().expect("failures");
            if *failures > 0 {
                *failures -= 1;
                return Err(UploadPartError::Http {
                    part_number: request.part.part_number,
                    status: self.status,
                    response_body: "server error".to_owned(),
                });
            }
            Ok(CompletedUploadPart {
                part_number: request.part.part_number,
                etag: format!("\"etag-{}\"", request.part.part_number),
            })
        }
    }

    fn archive(path: PathBuf, size: u64) -> FeedbackArchive {
        FeedbackArchive {
            path,
            size,
            sha256: "hash".to_owned(),
            fingerprint: "fingerprint".to_owned(),
            file_count: 1,
            cleanup_dir: None,
        }
    }

    fn part(number: i64, size: u64) -> FeedbackUploadPart {
        FeedbackUploadPart {
            part_number: number,
            url: format!("https://example.test/part{number}"),
            method: "PUT".to_owned(),
            size,
        }
    }

    #[tokio::test]
    async fn rejects_oversized_archive_before_requesting_upload_urls() {
        let api = ApiMock::default();
        let error = upload_archive_with_transport(
            &api,
            &TransportMock {
                requests: Mutex::new(Vec::new()),
                failures_left: Mutex::new(0),
                status: 500,
            },
            &archive(PathBuf::from("unused.zip"), MAX_ARCHIVE_SIZE + 1),
            3,
            UploadArchiveOptions::new("repo.zip"),
        )
        .await
        .expect_err("too large");
        assert!(matches!(error, UploadArchiveError::ArchiveTooLarge { .. }));
        assert!(api.created.lock().expect("created").is_empty());
    }

    #[tokio::test]
    async fn sorts_parts_computes_ranges_reports_progress_and_completes_in_part_order() {
        let api = ApiMock {
            parts: vec![part(3, 2), part(1, 4), part(2, 3)],
            ..ApiMock::default()
        };
        let transport = TransportMock {
            requests: Mutex::new(Vec::new()),
            failures_left: Mutex::new(0),
            status: 500,
        };
        let progress = Arc::new(Mutex::new(Vec::new()));
        let progress_events = Arc::clone(&progress);
        let mut options = UploadArchiveOptions::new("repo.zip");
        options.concurrency = 1;
        options.on_progress = Some(Arc::new(move |uploaded, total| {
            progress_events
                .lock()
                .expect("progress")
                .push((uploaded, total));
        }));

        upload_archive_with_transport(
            &api,
            &transport,
            &archive(PathBuf::from("repo.zip"), 9),
            3,
            options,
        )
        .await
        .expect("upload");

        assert_eq!(
            api.created.lock().expect("created").as_slice(),
            [CreateFeedbackUploadUrlInput {
                feedback_id: 3,
                filename: "repo.zip".to_owned(),
                size: 9,
                sha256: "hash".to_owned(),
            }]
        );
        let requests = transport.requests.lock().expect("requests");
        assert_eq!(
            requests
                .iter()
                .map(|request| (request.part.part_number, request.start))
                .collect::<Vec<_>>(),
            [(1, 0), (2, 4), (3, 7)]
        );
        assert_eq!(
            *progress.lock().expect("progress"),
            [(4, 9), (7, 9), (9, 9)]
        );
        assert_eq!(
            api.completed.lock().expect("completed").as_slice(),
            [CompleteFeedbackUploadUrlInput {
                upload_id: 28,
                parts: vec![
                    CompletedUploadPart {
                        part_number: 1,
                        etag: "\"etag-1\"".to_owned(),
                    },
                    CompletedUploadPart {
                        part_number: 2,
                        etag: "\"etag-2\"".to_owned(),
                    },
                    CompletedUploadPart {
                        part_number: 3,
                        etag: "\"etag-3\"".to_owned(),
                    },
                ],
            }]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn retries_retryable_http_errors_after_exponential_delay() {
        let api = ApiMock {
            parts: vec![part(1, 5)],
            ..ApiMock::default()
        };
        let transport = TransportMock {
            requests: Mutex::new(Vec::new()),
            failures_left: Mutex::new(1),
            status: 500,
        };
        let archive = archive(PathBuf::from("repo.zip"), 5);
        let upload = upload_archive_with_transport(
            &api,
            &transport,
            &archive,
            3,
            UploadArchiveOptions::new("repo.zip"),
        );
        upload.await.expect("retry succeeds");
        assert_eq!(transport.requests.lock().expect("requests").len(), 2);
        assert_eq!(api.completed.lock().expect("completed").len(), 1);
    }

    #[tokio::test]
    async fn does_not_retry_non_retryable_http_errors_or_complete_upload() {
        let api = ApiMock {
            parts: vec![part(1, 5)],
            ..ApiMock::default()
        };
        let transport = TransportMock {
            requests: Mutex::new(Vec::new()),
            failures_left: Mutex::new(1),
            status: 400,
        };
        let error = upload_archive_with_transport(
            &api,
            &transport,
            &archive(PathBuf::from("repo.zip"), 5),
            3,
            UploadArchiveOptions::new("repo.zip"),
        )
        .await
        .expect_err("bad request");
        assert!(matches!(
            error,
            UploadArchiveError::Part(UploadPartError::Http { status: 400, .. })
        ));
        assert_eq!(transport.requests.lock().expect("requests").len(), 1);
        assert!(api.completed.lock().expect("completed").is_empty());
    }

    #[tokio::test]
    async fn reqwest_transport_uses_backend_method_content_length_range_and_etag() {
        let temp = std::env::temp_dir().join(format!("feedback-http-{}", uuid::Uuid::new_v4()));
        std::fs::write(&temp, b"hello").expect("archive");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            let mut received = Vec::new();
            let mut buffer = [0_u8; 1024];
            let header_end = loop {
                let count = socket.read(&mut buffer).expect("read request");
                assert_ne!(count, 0);
                received.extend_from_slice(&buffer[..count]);
                if let Some(position) = received.windows(4).position(|item| item == b"\r\n\r\n") {
                    break position + 4;
                }
            };
            while received.len() < header_end + 3 {
                let count = socket.read(&mut buffer).expect("read body");
                assert_ne!(count, 0);
                received.extend_from_slice(&buffer[..count]);
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nETag: \"etag-1\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .expect("response");
            (
                received[..header_end].to_vec(),
                received[header_end..header_end + 3].to_vec(),
            )
        });
        let request = PartUploadRequest {
            file_path: temp.clone(),
            part: FeedbackUploadPart {
                part_number: 1,
                url: format!("http://{address}/upload"),
                method: "POST".to_owned(),
                size: 3,
            },
            start: 1,
            timeout: Duration::from_secs(5),
        };

        let completed = ReqwestPartUploadTransport::default()
            .upload_part(request)
            .await
            .expect("upload");
        let (headers, body) = server.join().expect("server");
        let headers = String::from_utf8(headers)
            .expect("headers")
            .to_ascii_lowercase();
        assert!(headers.starts_with("post /upload http/1.1\r\n"));
        assert!(headers.contains("content-length: 3\r\n"));
        assert_eq!(body, b"ell");
        assert_eq!(completed.etag, "\"etag-1\"");
        let _ = std::fs::remove_file(temp);
    }

    #[tokio::test]
    async fn stalled_part_times_out_without_completing_upload() {
        let temp = std::env::temp_dir().join(format!("feedback-timeout-{}", uuid::Uuid::new_v4()));
        std::fs::write(&temp, b"hello").expect("archive");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (_socket, _) = listener.accept().expect("accept");
            std::thread::sleep(Duration::from_millis(100));
        });
        let api = ApiMock {
            parts: vec![FeedbackUploadPart {
                part_number: 1,
                url: format!("http://{address}/upload"),
                method: "PUT".to_owned(),
                size: 5,
            }],
            ..ApiMock::default()
        };
        let mut options = UploadArchiveOptions::new("repo.zip");
        options.timeout = Duration::from_millis(20);
        options.max_retries = 0;

        let error = upload_archive(&api, &archive(temp.clone(), 5), 3, options)
            .await
            .expect_err("timeout");

        assert!(matches!(
            error,
            UploadArchiveError::Part(UploadPartError::TimedOut { part_number: 1 })
        ));
        assert!(api.completed.lock().expect("completed").is_empty());
        server.join().expect("server");
        let _ = std::fs::remove_file(temp);
    }

    #[test]
    fn api_error_wrapper_preserves_source() {
        let error = FeedbackUploadApiError::new(TestError("backend"));
        assert_eq!(error.to_string(), "backend");
        assert_eq!(error.source().expect("source").to_string(), "backend");
    }
}
