//! One-shot localhost OAuth callback listener.
//!
//! Original: `agent/mcp/oauth/callback-server.ts`.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::_base::utils::abort::AbortSignal;

const SUCCESS_HTML: &str = concat!(
    "<!doctype html><html><head><meta charset=\"utf-8\"><title>Authorized</title></head>",
    "<body style=\"font-family:system-ui,sans-serif;padding:2rem;\"><h1>Sign-in complete</h1>",
    "<p>You can close this tab and return to kimi-code.</p></body></html>"
);
const ERROR_HTML: &str = concat!(
    "<!doctype html><html><head><meta charset=\"utf-8\"><title>OAuth error</title></head>",
    "<body style=\"font-family:system-ui,sans-serif;padding:2rem;\"><h1>Sign-in failed</h1>",
    "<p>The authorization server reported an error. Return to kimi-code for details.</p></body></html>"
);
const MAX_REQUEST_HEADER_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallbackResult {
    pub code: String,
    pub state: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CallbackServerError {
    #[error("OAuth callback server closed")]
    Closed,

    #[error("OAuth callback listener is already being awaited")]
    AlreadyWaiting,

    #[error("OAuth callback timed out")]
    TimedOut,

    #[error("OAuth flow aborted: {0}")]
    Aborted(String),

    #[error("OAuth error: {error}{description}")]
    AuthorizationError { error: String, description: String },

    #[error("OAuth callback missing authorization code")]
    MissingAuthorizationCode,

    #[error("failed to start OAuth callback listener: {0}")]
    Bind(#[source] std::io::Error),
}

/// A callback listener that completes after the first valid or failed callback.
pub struct CallbackServer {
    pub redirect_uri: String,
    receiver: Mutex<Option<oneshot::Receiver<Result<CallbackResult, CallbackServerError>>>>,
    cancellation: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
    closed: AtomicBool,
}

impl CallbackServer {
    // Original: startCallbackServer().
    pub async fn start() -> Result<Arc<Self>, CallbackServerError> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(CallbackServerError::Bind)?;
        let port = listener
            .local_addr()
            .map_err(CallbackServerError::Bind)?
            .port();
        let cancellation = CancellationToken::new();
        let (sender, receiver) = oneshot::channel();
        let task = tokio::spawn(run_listener(listener, sender, cancellation.clone()));
        Ok(Arc::new(Self {
            redirect_uri: format!("http://127.0.0.1:{port}/callback"),
            receiver: Mutex::new(Some(receiver)),
            cancellation,
            task: Mutex::new(Some(task)),
            closed: AtomicBool::new(false),
        }))
    }

    // Original: CallbackServer.waitForCode().
    pub async fn wait_for_code(
        &self,
        signal: Option<AbortSignal>,
        timeout_ms: Option<u64>,
    ) -> Result<CallbackResult, CallbackServerError> {
        let receiver = self
            .receiver
            .lock()
            .await
            .take()
            .ok_or(CallbackServerError::AlreadyWaiting)?;
        let wait = async { receiver.await.unwrap_or(Err(CallbackServerError::Closed)) };
        let result = match (signal, timeout_ms) {
            (Some(signal), Some(timeout_ms)) => {
                tokio::select! {
                    result = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), wait) => {
                        result.map_err(|_| CallbackServerError::TimedOut).and_then(|result| result)
                    }
                    error = signal.cancelled() => Err(CallbackServerError::Aborted(error.to_string())),
                }
            }
            (Some(signal), None) => {
                tokio::select! {
                    result = wait => result,
                    error = signal.cancelled() => Err(CallbackServerError::Aborted(error.to_string())),
                }
            }
            (None, Some(timeout_ms)) => {
                tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), wait)
                    .await
                    .map_err(|_| CallbackServerError::TimedOut)
                    .and_then(|result| result)
            }
            (None, None) => wait.await,
        };
        self.close().await;
        result
    }

    // Original: CallbackServer.close().
    pub async fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.cancellation.cancel();
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
    }
}

impl Drop for CallbackServer {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

async fn run_listener(
    listener: TcpListener,
    sender: oneshot::Sender<Result<CallbackResult, CallbackServerError>>,
    cancellation: CancellationToken,
) {
    let mut sender = Some(sender);
    loop {
        let accept = listener.accept();
        let accepted = tokio::select! {
            _ = cancellation.cancelled() => return,
            accepted = accept => accepted,
        };
        let Ok((stream, _)) = accepted else {
            return;
        };
        let outcome = handle_callback_request(stream).await;
        match outcome {
            CallbackOutcome::Ignore => {}
            CallbackOutcome::Settle(result) => {
                if let Some(sender) = sender.take() {
                    let _ = sender.send(result);
                }
                cancellation.cancel();
                return;
            }
        }
    }
}

enum CallbackOutcome {
    Ignore,
    Settle(Result<CallbackResult, CallbackServerError>),
}

async fn handle_callback_request(mut stream: TcpStream) -> CallbackOutcome {
    let Some(request_target) = read_request_target(&mut stream).await else {
        write_response(&mut stream, 404, "", "").await;
        return CallbackOutcome::Ignore;
    };
    let Ok(url) = Url::parse(&format!("http://localhost{request_target}")) else {
        write_response(&mut stream, 404, "", "").await;
        return CallbackOutcome::Ignore;
    };
    if url.path() != "/callback" {
        write_response(&mut stream, 404, "", "").await;
        return CallbackOutcome::Ignore;
    }
    if let Some(error) = url
        .query_pairs()
        .find_map(|(name, value)| (name == "error").then_some(value.into_owned()))
    {
        let description = url
            .query_pairs()
            .find_map(|(name, value)| (name == "error_description").then_some(value.into_owned()));
        write_response(&mut stream, 400, "text/html; charset=utf-8", ERROR_HTML).await;
        let description = description.map_or_else(String::new, |value| format!(" — {value}"));
        return CallbackOutcome::Settle(Err(CallbackServerError::AuthorizationError {
            error,
            description,
        }));
    }
    let code = url
        .query_pairs()
        .find_map(|(name, value)| (name == "code").then_some(value.into_owned()));
    let Some(code) = code.filter(|code| !code.is_empty()) else {
        write_response(&mut stream, 400, "text/html; charset=utf-8", ERROR_HTML).await;
        return CallbackOutcome::Settle(Err(CallbackServerError::MissingAuthorizationCode));
    };
    let state = url
        .query_pairs()
        .find_map(|(name, value)| (name == "state").then_some(value.into_owned()));
    write_response(&mut stream, 200, "text/html; charset=utf-8", SUCCESS_HTML).await;
    CallbackOutcome::Settle(Ok(CallbackResult { code, state }))
}

async fn read_request_target(stream: &mut TcpStream) -> Option<String> {
    let mut bytes = Vec::with_capacity(1024);
    let mut buffer = [0_u8; 1024];
    while bytes.len() < MAX_REQUEST_HEADER_BYTES {
        let read = stream.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let header = std::str::from_utf8(&bytes).ok()?;
    let request_line = header.lines().next()?;
    let mut parts = request_line.split_whitespace();
    (parts.next()? == "GET").then_some(parts.next()?.to_owned())
}

async fn write_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Not Found",
    };
    let content_type = if content_type.is_empty() {
        String::new()
    } else {
        format!("Content-Type: {content_type}\r\n")
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n{content_type}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn request(uri: &str) -> String {
        let url = Url::parse(uri).unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", url.port().unwrap()))
            .await
            .unwrap();
        let path = format!(
            "{}{}",
            url.path(),
            url.query()
                .map_or(String::new(), |query| format!("?{query}"))
        );
        stream
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response
    }

    #[tokio::test]
    async fn accepts_first_callback_code_and_ignores_non_callback_requests() {
        let server = CallbackServer::start().await.unwrap();
        assert!(
            request(&server.redirect_uri.replace("/callback", "/other"))
                .await
                .starts_with("HTTP/1.1 404")
        );
        let response = request(&format!("{}?code=abc&state=expected", server.redirect_uri)).await;
        assert!(response.starts_with("HTTP/1.1 200"));
        let result = server.wait_for_code(None, Some(500)).await.unwrap();
        assert_eq!(
            result,
            CallbackResult {
                code: "abc".into(),
                state: Some("expected".into())
            }
        );
    }

    #[tokio::test]
    async fn reports_callback_errors_and_wait_timeouts() {
        let server = CallbackServer::start().await.unwrap();
        let response = request(&format!(
            "{}?error=access_denied&error_description=nope",
            server.redirect_uri
        ))
        .await;
        assert!(response.starts_with("HTTP/1.1 400"));
        assert_eq!(
            server
                .wait_for_code(None, Some(500))
                .await
                .unwrap_err()
                .to_string(),
            "OAuth error: access_denied — nope"
        );

        let server = CallbackServer::start().await.unwrap();
        assert_eq!(
            server
                .wait_for_code(None, Some(1))
                .await
                .unwrap_err()
                .to_string(),
            "OAuth callback timed out"
        );
    }
}
