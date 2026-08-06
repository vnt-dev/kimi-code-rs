//! Shared host helpers for capability entries: process execution with
//! captured output, and streaming downloads with progress reporting.
//!
//! `run_command` never throws for an expected failure — a spawn failure or a
//! non-zero exit resolves into the result (`code: -1` for spawn failures),
//! while a timeout kills the process and rejects. `download_to_file` bounds
//! both the response-header wait and stream inactivity (a watchdog reset per
//! chunk, 30s by default), so a stalled CDN connection fails the background
//! install instead of wedging it.
//!
//! Original: `packages/agent-core-v2/src/app/capability/host.ts`.

use std::{
    error::Error,
    io::{self, ErrorKind},
    path::Path,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{
    _base::errors::errors::ExpectedError,
    os::interface::host_process::{HostProcess, HostProcessOptions, HostProcessServiceHandle, SharedProcessReader},
};

pub const DOWNLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

// Original: runCommand(). A spawn failure resolves (`code: -1`) instead of
// throwing; only the caller's own timeout rejects, after killing the process.
pub async fn run_command(
    host_process: &HostProcessServiceHandle,
    command: &str,
    args: &[String],
    timeout: Option<Duration>,
) -> Result<CommandResult, Box<dyn Error + Send + Sync>> {
    let proc = match host_process
        .spawn(
            command,
            args,
            HostProcessOptions {
                windows_hide: Some(true),
                ..HostProcessOptions::default()
            },
        )
        .await
    {
        Ok(proc) => proc,
        Err(error) => {
            return Ok(CommandResult {
                code: -1,
                stdout: String::new(),
                stderr: error.to_string(),
            });
        }
    };
    let result = match timeout {
        Some(timeout) => match tokio::time::timeout(timeout, collect_process_output(&*proc)).await {
            Ok(output) => output,
            Err(_) => {
                let _ = proc.kill(None).await;
                Err(Box::new(ExpectedError::new(format!(
                    "command timed out after {}ms: {command}",
                    timeout.as_millis()
                ))) as Box<dyn Error + Send + Sync>)
            }
        },
        None => collect_process_output(&*proc).await,
    };
    proc.dispose();
    result.map(|(stdout, stderr, code)| CommandResult {
        code,
        stdout,
        stderr,
    })
}

async fn collect_process_output(
    proc: &dyn HostProcess,
) -> Result<(String, String, i32), Box<dyn Error + Send + Sync>> {
    let stdout = proc.stdout();
    let stderr = proc.stderr();
    let (stdout, stderr, code) = tokio::join!(
        collect(stdout),
        collect(stderr),
        async { proc.wait().await.unwrap_or(-1) }
    );
    Ok((stdout?, stderr?, code))
}

// Original: collect() — read a process stream to EOF as UTF-8 text.
async fn collect(stream: SharedProcessReader) -> io::Result<String> {
    let mut bytes = Vec::new();
    stream.lock().await.read_to_end(&mut bytes).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Original: `FetchLike` — the minimal fetch surface used by capability
/// entries, injectable so tests can script responses instead of touching the
/// network.
#[async_trait]
pub trait FetchLike: Send + Sync {
    async fn fetch(&self, url: &str) -> Result<FetchResponse, Box<dyn Error + Send + Sync>>;
}

pub struct FetchResponse {
    pub ok: bool,
    pub status: u16,
    pub content_length: Option<u64>,
    pub body: Option<FetchBodyStream>,
}

pub type FetchBodyStream =
    Pin<Box<dyn Stream<Item = Result<Vec<u8>, Box<dyn Error + Send + Sync>>> + Send>>;

/// Default `FetchLike` backed by reqwest.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReqwestFetch;

#[async_trait]
impl FetchLike for ReqwestFetch {
    async fn fetch(&self, url: &str) -> Result<FetchResponse, Box<dyn Error + Send + Sync>> {
        let response = reqwest::get(url)
            .await
            .map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync>)?;
        let status = response.status();
        let content_length = response.content_length();
        let body = response.bytes_stream().map(|chunk| {
            chunk
                .map(|bytes| bytes.to_vec())
                .map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync>)
        });
        Ok(FetchResponse {
            ok: status.is_success(),
            status: status.as_u16(),
            content_length,
            body: Some(Box::pin(body)),
        })
    }
}

// Original: downloadToFile(). The header wait and the per-chunk idle watchdog
// share one budget; the header deadline must not abort a flowing body.
pub async fn download_to_file(
    url: &str,
    dest_path: &Path,
    on_percent: Option<&(dyn Fn(u32) + Send + Sync)>,
    fetch_impl: &Arc<dyn FetchLike>,
    idle_timeout: Option<Duration>,
) -> Result<u64, Box<dyn Error + Send + Sync>> {
    let idle_timeout = idle_timeout.unwrap_or(DOWNLOAD_IDLE_TIMEOUT);
    let response = match tokio::time::timeout(idle_timeout, fetch_impl.fetch(url)).await {
        Ok(response) => response?,
        Err(_) => {
            return Err(Box::new(ExpectedError::new(format!(
                "Failed to download {url}: no response within {}ms",
                idle_timeout.as_millis()
            ))));
        }
    };
    let mut body = match response.body {
        Some(body) if response.ok => body,
        _ => {
            return Err(Box::new(ExpectedError::new(format!(
                "Failed to download {url}: HTTP {}",
                response.status
            ))));
        }
    };
    let total = response.content_length.unwrap_or(0);
    if let Some(parent) = dest_path.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = tokio::fs::File::create(dest_path).await?;
    let mut received = 0_u64;
    loop {
        let chunk = match tokio::time::timeout(idle_timeout, body.next()).await {
            Ok(Some(chunk)) => chunk?,
            Ok(None) => break,
            Err(_) => {
                return Err(Box::new(ExpectedError::new(format!(
                    "Download stalled for {}ms: {url}",
                    idle_timeout.as_millis()
                ))));
            }
        };
        file.write_all(&chunk).await?;
        received += chunk.len() as u64;
        if let Some(on_percent) = on_percent
            && let Some(percent) = (received * 100).checked_div(total)
        {
            on_percent(percent.min(99) as u32);
        }
    }
    file.flush().await?;
    if let Some(on_percent) = on_percent {
        on_percent(100);
    }
    Ok(received)
}

/// Original: node `rm(path, { recursive: true, force: true })` — removes a
/// file or directory tree, tolerating a missing path.
pub async fn rm_force(path: &Path) -> io::Result<()> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(dir_error) => match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(_) => Err(dir_error),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::{
        pin::Pin,
        sync::Mutex,
        task::{Context, Poll},
    };

    use futures_util::stream;
    use tokio::{
        io::{AsyncRead, ReadBuf},
        sync::Mutex as AsyncMutex,
    };

    use crate::{
        _base::errors::errors::Error2Options,
        os::interface::host_process::{
            HOST_PROCESS_SERVICE_ID, HostProcessError, HostProcessService, OS_PROCESS_SPAWN_FAILED,
            ProcessSignal, SharedProcessWriter,
        },
    };

    use super::*;

    struct PendingReader;

    impl AsyncRead for PendingReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Pending
        }
    }

    struct FailingReader;

    impl AsyncRead for FailingReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "stream closed after timeout",
            )))
        }
    }

    fn data_reader(bytes: &[u8]) -> SharedProcessReader {
        Arc::new(AsyncMutex::new(
            Box::new(std::io::Cursor::new(bytes.to_vec())) as Box<dyn AsyncRead + Send + Unpin>
        ))
    }

    struct ScriptedProc {
        code: i32,
        stdout: SharedProcessReader,
        stderr: SharedProcessReader,
        wait_hangs: bool,
        kill_calls: Arc<Mutex<Vec<Option<ProcessSignal>>>>,
    }

    #[async_trait]
    impl HostProcess for ScriptedProc {
        fn pid(&self) -> i64 {
            1234
        }
        fn exit_code(&self) -> Option<i32> {
            Some(self.code)
        }
        fn stdin(&self) -> SharedProcessWriter {
            Arc::new(AsyncMutex::new(
                Box::new(tokio::io::sink()) as Box<dyn tokio::io::AsyncWrite + Send + Unpin>
            ))
        }
        fn stdout(&self) -> SharedProcessReader {
            Arc::clone(&self.stdout)
        }
        fn stderr(&self) -> SharedProcessReader {
            Arc::clone(&self.stderr)
        }
        async fn wait(&self) -> Result<i32, HostProcessError> {
            if self.wait_hangs {
                std::future::pending::<()>().await;
            }
            Ok(self.code)
        }
        async fn kill(&self, signal: Option<ProcessSignal>) -> Result<(), HostProcessError> {
            self.kill_calls.lock().unwrap().push(signal);
            Ok(())
        }
        fn dispose(&self) {}
    }

    struct ScriptedSpawn {
        proc: Option<Arc<ScriptedProc>>,
    }

    #[async_trait]
    impl HostProcessService for ScriptedSpawn {
        async fn spawn(
            &self,
            _: &str,
            _: &[String],
            _: HostProcessOptions,
        ) -> Result<Arc<dyn HostProcess>, HostProcessError> {
            match &self.proc {
                Some(proc) => Ok(Arc::clone(proc) as Arc<dyn HostProcess>),
                None => Err(HostProcessError::with_options(
                    OS_PROCESS_SPAWN_FAILED,
                    "spawn hang ENOENT",
                    Error2Options::default(),
                )),
            }
        }
    }

    fn host(proc: Option<Arc<ScriptedProc>>) -> HostProcessServiceHandle {
        HostProcessServiceHandle(Arc::new(ScriptedSpawn { proc }))
    }

    fn scripted_proc(code: i32, stdout: &[u8], stderr: &[u8]) -> Arc<ScriptedProc> {
        Arc::new(ScriptedProc {
            code,
            stdout: data_reader(stdout),
            stderr: data_reader(stderr),
            wait_hangs: false,
            kill_calls: Arc::new(Mutex::new(Vec::new())),
        })
    }

    #[tokio::test]
    async fn collects_output_and_exit_code() {
        let proc = scripted_proc(7, b"out", b"err");
        let result = run_command(&host(Some(proc)), "cmd", &[], None).await.unwrap();
        assert_eq!(
            result,
            CommandResult {
                code: 7,
                stdout: "out".to_owned(),
                stderr: "err".to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn spawn_failure_resolves_code_minus_one() {
        let result = run_command(&host(None), "missing", &[], None).await.unwrap();
        assert_eq!(result.code, -1);
        assert_eq!(result.stdout, "");
        assert!(result.stderr.contains("ENOENT"));
    }

    // Original: host.test.ts — a timed-out process whose streams and wait
    // fail while being killed must still surface only the timeout error.
    #[tokio::test]
    async fn timed_out_command_kills_and_rejects() {
        let kill_calls = Arc::new(Mutex::new(Vec::new()));
        let proc = Arc::new(ScriptedProc {
            code: -1,
            stdout: Arc::new(AsyncMutex::new(
                Box::new(FailingReader) as Box<dyn AsyncRead + Send + Unpin>
            )),
            stderr: Arc::new(AsyncMutex::new(
                Box::new(PendingReader) as Box<dyn AsyncRead + Send + Unpin>
            )),
            wait_hangs: true,
            kill_calls: Arc::clone(&kill_calls),
        });
        let error = run_command(
            &host(Some(proc)),
            "hang",
            &[],
            Some(Duration::from_millis(5)),
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "command timed out after 5ms: hang");
        assert_eq!(kill_calls.lock().unwrap().len(), 1);
    }

    struct ScriptedFetch {
        hang: bool,
        ok: bool,
        status: u16,
        content_length: Option<u64>,
        body: Mutex<Option<Option<FetchBodyStream>>>,
    }

    impl ScriptedFetch {
        fn respond(
            status: u16,
            content_length: Option<u64>,
            body: Option<FetchBodyStream>,
        ) -> Arc<Self> {
            Arc::new(Self {
                hang: false,
                ok: (200..300).contains(&status),
                status,
                content_length,
                body: Mutex::new(Some(body)),
            })
        }

        fn hanging() -> Arc<Self> {
            Arc::new(Self {
                hang: true,
                ok: false,
                status: 0,
                content_length: None,
                body: Mutex::new(None),
            })
        }
    }

    #[async_trait]
    impl FetchLike for ScriptedFetch {
        async fn fetch(&self, _: &str) -> Result<FetchResponse, Box<dyn Error + Send + Sync>> {
            if self.hang {
                return std::future::pending().await;
            }
            Ok(FetchResponse {
                ok: self.ok,
                status: self.status,
                content_length: self.content_length,
                body: self.body.lock().unwrap().take().flatten(),
            })
        }
    }

    fn chunk_stream(chunks: Vec<&'static [u8]>, stall_after: bool) -> FetchBodyStream {
        type Item = Result<Vec<u8>, Box<dyn Error + Send + Sync>>;
        let items = stream::iter(chunks.into_iter().map(|chunk| Ok(chunk.to_vec()) as Item));
        if stall_after {
            Box::pin(items.chain(stream::pending()))
        } else {
            Box::pin(items)
        }
    }

    async fn temp_root(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("capability-{tag}-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        root
    }

    #[tokio::test]
    async fn download_aborts_a_response_whose_byte_stream_goes_quiet() {
        let root = temp_root("download").await;
        let fetch: Arc<dyn FetchLike> = ScriptedFetch::respond(
            200,
            Some(100),
            Some(chunk_stream(vec![&[1, 2, 3]], true)),
        );
        let error = download_to_file(
            "https://cdn.example.test/blob",
            &root.join("blob"),
            None,
            &fetch,
            Some(Duration::from_millis(5)),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("stalled"), "{error}");
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn download_aborts_when_the_response_headers_never_arrive() {
        let root = temp_root("download").await;
        let fetch: Arc<dyn FetchLike> = ScriptedFetch::hanging();
        let error = download_to_file(
            "https://cdn.example.test/headers",
            &root.join("headers"),
            None,
            &fetch,
            Some(Duration::from_millis(5)),
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("no response within 5ms"),
            "{error}"
        );
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn download_lets_a_slow_but_flowing_download_finish_intact() {
        let root = temp_root("download").await;
        let chunks: Vec<&[u8]> = vec![b"hel", b"lo ", b"wor", b"ld"];
        type Item = Result<Vec<u8>, Box<dyn Error + Send + Sync>>;
        let body: FetchBodyStream = Box::pin(stream::iter(chunks).then(|chunk| async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            Ok(chunk.to_vec()) as Item
        }));
        let fetch: Arc<dyn FetchLike> = ScriptedFetch::respond(200, Some(11), Some(body));
        let dest = root.join("hello.txt");
        let received = download_to_file(
            "https://cdn.example.test/hello",
            &dest,
            None,
            &fetch,
            Some(Duration::from_millis(50)),
        )
        .await
        .unwrap();
        assert_eq!(received, 11);
        assert_eq!(tokio::fs::read_to_string(&dest).await.unwrap(), "hello world");
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn download_reports_capped_progress_per_chunk_then_100() {
        let root = temp_root("download").await;
        let percents = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&percents);
        let fetch: Arc<dyn FetchLike> = ScriptedFetch::respond(
            200,
            Some(100),
            Some(chunk_stream(vec![&[0_u8; 40], &[0_u8; 40], &[0_u8; 20]], false)),
        );
        let received = download_to_file(
            "https://cdn.example.test/blob",
            &root.join("blob"),
            Some(&move |percent| recorded.lock().unwrap().push(percent)),
            &fetch,
            None,
        )
        .await
        .unwrap();
        assert_eq!(received, 100);
        assert_eq!(*percents.lock().unwrap(), vec![40, 80, 99, 100]);
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn download_rejects_http_errors_and_missing_bodies() {
        let root = temp_root("download").await;
        let fetch: Arc<dyn FetchLike> = ScriptedFetch::respond(500, None, None);
        let error = download_to_file(
            "https://cdn.example.test/missing",
            &root.join("missing"),
            None,
            &fetch,
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Failed to download https://cdn.example.test/missing: HTTP 500"
        );
        let fetch: Arc<dyn FetchLike> = ScriptedFetch::respond(200, None, None);
        let error = download_to_file(
            "https://cdn.example.test/empty",
            &root.join("empty"),
            None,
            &fetch,
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Failed to download https://cdn.example.test/empty: HTTP 200"
        );
        rm_force(&root).await.unwrap();
    }

    #[test]
    fn host_process_service_identifier_matches_the_original() {
        assert_eq!(HOST_PROCESS_SERVICE_ID.to_string(), "hostProcessService");
    }
}
