use std::{error::Error, fmt};

use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartVisServerArgs {
    pub home_dir: String,
    pub port: u16,
    pub host: Option<String>,
    pub web_asset: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisOptions {
    pub open: bool,
    pub port: Option<u16>,
    pub host: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisDisposition {
    Completed,
    Exit(i32),
}

#[derive(Debug)]
pub struct VisRuntimeError(Box<dyn Error + Send + Sync>);

impl VisRuntimeError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for VisRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for VisRuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[async_trait]
pub trait StartedVisServer: Send {
    fn port(&self) -> u16;

    fn host(&self) -> &str;

    fn url(&self) -> &str;

    async fn close(&mut self) -> Result<(), VisRuntimeError>;
}

#[async_trait]
pub trait VisRuntime: Send + Sync {
    fn home_dir(&self) -> String;

    fn embedded_web_asset(&self) -> Option<Vec<u8>>;

    async fn start_vis_server(
        &self,
        options: StartVisServerArgs,
    ) -> Result<Box<dyn StartedVisServer>, VisRuntimeError>;

    async fn open_url(&self, url: &str) -> Result<(), VisRuntimeError>;

    async fn wait_for_shutdown(&self) -> Result<(), VisRuntimeError>;

    fn write_stdout(&self, text: &str);

    fn write_stderr(&self, text: &str);
}

// Original:
//   apps/kimi-code/src/cli/sub/vis.ts
//   handleVis()
//
// Rust adaptation:
//   A requested process exit is returned to the binary entrypoint as a
//   disposition. Server lifecycle and error ordering otherwise stay in the
//   handler, including the best-effort browser open.
pub async fn handle_vis(
    runtime: &dyn VisRuntime,
    options: &VisOptions,
) -> Result<VisDisposition, VisRuntimeError> {
    let start_options = StartVisServerArgs {
        home_dir: runtime.home_dir(),
        port: options.port.unwrap_or(0),
        host: options.host.clone(),
        web_asset: runtime.embedded_web_asset(),
    };
    let mut server = match runtime.start_vis_server(start_options).await {
        Ok(server) => server,
        Err(error) => {
            runtime.write_stderr(&format!("Failed to start kimi vis: {error}\n"));
            return Ok(VisDisposition::Exit(1));
        }
    };

    let target = options.session_id.as_deref().map_or_else(
        || server.url().to_owned(),
        |session_id| {
            let encoded = encode_uri_component(session_id);
            format!("{}sessions/{encoded}", server.url())
        },
    );
    runtime.write_stdout(&format!("kimi vis is running at {}\n", server.url()));
    runtime.write_stdout("Press Ctrl-C to stop.\n");

    if options.open && runtime.open_url(&target).await.is_err() {
        runtime.write_stderr(&format!(
            "Could not open a browser; visit {target} manually.\n"
        ));
    }

    runtime.wait_for_shutdown().await?;
    server.close().await?;
    Ok(VisDisposition::Completed)
}

fn encode_uri_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            )
        {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
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

    struct ServerMock {
        close_called: Arc<AtomicBool>,
        close_error: bool,
    }

    #[async_trait]
    impl StartedVisServer for ServerMock {
        fn port(&self) -> u16 {
            41_234
        }

        fn host(&self) -> &str {
            "127.0.0.1"
        }

        fn url(&self) -> &str {
            "http://127.0.0.1:41234/"
        }

        async fn close(&mut self) -> Result<(), VisRuntimeError> {
            self.close_called.store(true, Ordering::SeqCst);
            if self.close_error {
                Err(VisRuntimeError::new(TestError("close failed")))
            } else {
                Ok(())
            }
        }
    }

    struct RuntimeMock {
        start_options: Mutex<Vec<StartVisServerArgs>>,
        opened: Mutex<Vec<String>>,
        stdout: Mutex<String>,
        stderr: Mutex<String>,
        close_called: Arc<AtomicBool>,
        start_error: bool,
        open_error: bool,
    }

    impl RuntimeMock {
        fn new() -> Self {
            Self {
                start_options: Mutex::new(Vec::new()),
                opened: Mutex::new(Vec::new()),
                stdout: Mutex::new(String::new()),
                stderr: Mutex::new(String::new()),
                close_called: Arc::new(AtomicBool::new(false)),
                start_error: false,
                open_error: false,
            }
        }
    }

    #[async_trait]
    impl VisRuntime for RuntimeMock {
        fn home_dir(&self) -> String {
            "/home/k".to_owned()
        }

        fn embedded_web_asset(&self) -> Option<Vec<u8>> {
            None
        }

        async fn start_vis_server(
            &self,
            options: StartVisServerArgs,
        ) -> Result<Box<dyn StartedVisServer>, VisRuntimeError> {
            self.start_options
                .lock()
                .expect("start options")
                .push(options);
            if self.start_error {
                return Err(VisRuntimeError::new(TestError(
                    "listen EADDRINUSE: address already in use 127.0.0.1:4321",
                )));
            }
            Ok(Box::new(ServerMock {
                close_called: Arc::clone(&self.close_called),
                close_error: false,
            }))
        }

        async fn open_url(&self, url: &str) -> Result<(), VisRuntimeError> {
            self.opened.lock().expect("opened").push(url.to_owned());
            if self.open_error {
                Err(VisRuntimeError::new(TestError("browser failed")))
            } else {
                Ok(())
            }
        }

        async fn wait_for_shutdown(&self) -> Result<(), VisRuntimeError> {
            Ok(())
        }

        fn write_stdout(&self, text: &str) {
            self.stdout.lock().expect("stdout").push_str(text);
        }

        fn write_stderr(&self, text: &str) {
            self.stderr.lock().expect("stderr").push_str(text);
        }
    }

    #[tokio::test]
    async fn starts_auto_port_opens_browser_and_closes_after_shutdown() {
        let runtime = RuntimeMock::new();
        let disposition = handle_vis(
            &runtime,
            &VisOptions {
                open: true,
                port: None,
                host: None,
                session_id: None,
            },
        )
        .await
        .expect("vis");

        assert_eq!(disposition, VisDisposition::Completed);
        assert_eq!(runtime.start_options.lock().expect("options")[0].port, 0);
        assert_eq!(
            runtime.opened.lock().expect("opened").as_slice(),
            ["http://127.0.0.1:41234/"]
        );
        assert!(runtime.close_called.load(Ordering::SeqCst));
        assert!(
            runtime
                .stdout
                .lock()
                .expect("stdout")
                .contains("http://127.0.0.1:41234/")
        );
    }

    #[tokio::test]
    async fn honors_explicit_bind_options_and_no_open() {
        let runtime = RuntimeMock::new();
        handle_vis(
            &runtime,
            &VisOptions {
                open: false,
                port: Some(4_321),
                host: Some("0.0.0.0".to_owned()),
                session_id: None,
            },
        )
        .await
        .expect("vis");
        let starts = runtime.start_options.lock().expect("options");
        assert_eq!(starts[0].port, 4_321);
        assert_eq!(starts[0].host.as_deref(), Some("0.0.0.0"));
        assert!(runtime.opened.lock().expect("opened").is_empty());
    }

    #[tokio::test]
    async fn encodes_session_deep_link_like_encode_uri_component() {
        assert_eq!(
            encode_uri_component("会话 é"),
            "%E4%BC%9A%E8%AF%9D%20%C3%A9"
        );
        let runtime = RuntimeMock::new();
        handle_vis(
            &runtime,
            &VisOptions {
                open: true,
                port: None,
                host: None,
                session_id: Some("sess a/b?".to_owned()),
            },
        )
        .await
        .expect("vis");
        assert_eq!(
            runtime.opened.lock().expect("opened").as_slice(),
            ["http://127.0.0.1:41234/sessions/sess%20a%2Fb%3F"]
        );
    }

    #[tokio::test]
    async fn browser_failure_prints_manual_url_and_still_closes() {
        let mut runtime = RuntimeMock::new();
        runtime.open_error = true;
        let disposition = handle_vis(
            &runtime,
            &VisOptions {
                open: true,
                port: None,
                host: None,
                session_id: None,
            },
        )
        .await
        .expect("vis");
        assert_eq!(disposition, VisDisposition::Completed);
        assert!(
            runtime
                .stderr
                .lock()
                .expect("stderr")
                .contains("visit http://127.0.0.1:41234/ manually")
        );
        assert!(runtime.close_called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn startup_failure_reports_clean_error_and_returns_exit_one() {
        let mut runtime = RuntimeMock::new();
        runtime.start_error = true;
        let disposition = handle_vis(
            &runtime,
            &VisOptions {
                open: true,
                port: Some(4_321),
                host: None,
                session_id: None,
            },
        )
        .await
        .expect("handled startup error");
        assert_eq!(disposition, VisDisposition::Exit(1));
        assert!(
            runtime
                .stderr
                .lock()
                .expect("stderr")
                .contains("EADDRINUSE")
        );
        assert!(runtime.opened.lock().expect("opened").is_empty());
        assert!(!runtime.close_called.load(Ordering::SeqCst));
    }
}
