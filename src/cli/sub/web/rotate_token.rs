use std::{error::Error, fmt};

use async_trait::async_trait;

use super::access_urls::{access_url_lines, split_token_fragment};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveServerInstance {
    pub host: String,
    pub port: u16,
}

#[derive(Debug)]
pub struct RotateTokenError(Box<dyn Error + Send + Sync>);

impl RotateTokenError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for RotateTokenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for RotateTokenError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[async_trait]
pub trait RotateTokenRuntime: Send + Sync {
    async fn rotate_server_token(&self) -> Result<String, RotateTokenError>;

    async fn live_server_instance(&self) -> Result<Option<LiveServerInstance>, RotateTokenError>;

    fn color_enabled(&self) -> bool;

    fn write_stdout(&self, text: &str);
}

// Original:
//   apps/kimi-code/src/cli/sub/web/rotate-token.ts
//   registerRotateTokenCommand().action()
pub async fn handle_rotate_token(runtime: &dyn RotateTokenRuntime) -> Result<(), RotateTokenError> {
    let token = runtime.rotate_server_token().await?;
    runtime.write_stdout(
        "The previous token is now invalid. A running server picks up the new token automatically.\n",
    );
    let heading = if runtime.color_enabled() {
        "\u{1b}[1mNew server token:\u{1b}[22m".to_owned()
    } else {
        "New server token:".to_owned()
    };
    runtime.write_stdout(&format!("\n  {heading} {token}\n\n"));

    if let Some(instance) = runtime.live_server_instance().await? {
        for line in access_url_lines(&instance.host, instance.port, Some(&token), None) {
            let (base, fragment) = split_token_fragment(&line.url);
            let rendered = if runtime.color_enabled() {
                format!(
                    "\u{1b}[38;2;91;192;190m{base}\u{1b}[0m\u{1b}[38;2;136;136;136m{fragment}\u{1b}[0m"
                )
            } else {
                line.url
            };
            let label = if runtime.color_enabled() {
                format!("\u{1b}[2m{}\u{1b}[22m", line.label)
            } else {
                line.label.to_owned()
            };
            runtime.write_stdout(&format!("  {label}{rendered}\n"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Debug, Clone, Copy)]
    struct TestError;

    impl fmt::Display for TestError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("rotation failed")
        }
    }

    impl Error for TestError {}

    struct RuntimeMock {
        token: String,
        instance: Option<LiveServerInstance>,
        color: bool,
        fail_rotation: bool,
        calls: Mutex<Vec<&'static str>>,
        stdout: Mutex<String>,
    }

    impl RuntimeMock {
        fn new() -> Self {
            Self {
                token: "new-token-abcdefghijklmnopqrstuvwxyz".to_owned(),
                instance: None,
                color: false,
                fail_rotation: false,
                calls: Mutex::new(Vec::new()),
                stdout: Mutex::new(String::new()),
            }
        }
    }

    #[async_trait]
    impl RotateTokenRuntime for RuntimeMock {
        async fn rotate_server_token(&self) -> Result<String, RotateTokenError> {
            self.calls.lock().expect("calls").push("rotate");
            if self.fail_rotation {
                Err(RotateTokenError::new(TestError))
            } else {
                Ok(self.token.clone())
            }
        }

        async fn live_server_instance(
            &self,
        ) -> Result<Option<LiveServerInstance>, RotateTokenError> {
            self.calls.lock().expect("calls").push("instance");
            Ok(self.instance.clone())
        }

        fn color_enabled(&self) -> bool {
            self.color
        }

        fn write_stdout(&self, text: &str) {
            self.stdout.lock().expect("stdout").push_str(text);
        }
    }

    #[tokio::test]
    async fn rotates_then_prints_the_new_token_and_restart_notice() {
        let runtime = RuntimeMock::new();
        handle_rotate_token(&runtime).await.expect("rotate");

        assert_eq!(
            runtime.calls.lock().expect("calls").as_slice(),
            ["rotate", "instance"]
        );
        let output = runtime.stdout.lock().expect("stdout");
        assert!(output.contains("previous token is now invalid"));
        assert!(output.contains("running server picks up the new token automatically"));
        assert!(output.contains("New server token: new-token-abcdefghijklmnopqrstuvwxyz"));
    }

    #[tokio::test]
    async fn live_instance_reprints_access_link_after_the_token() {
        let mut runtime = RuntimeMock::new();
        runtime.instance = Some(LiveServerInstance {
            host: "127.0.0.1".to_owned(),
            port: 58_627,
        });
        handle_rotate_token(&runtime).await.expect("rotate");

        let output = runtime.stdout.lock().expect("stdout");
        let heading = output.find("New server token:").expect("heading");
        let link = output
            .find("http://127.0.0.1:58627/#token=new-token-abcdefghijklmnopqrstuvwxyz")
            .expect("link");
        assert!(heading < link);
        assert!(output.contains("Local:"));
    }

    #[tokio::test]
    async fn wildcard_instance_reprints_local_and_network_links() {
        let mut runtime = RuntimeMock::new();
        runtime.instance = Some(LiveServerInstance {
            host: "0.0.0.0".to_owned(),
            port: 58_627,
        });
        handle_rotate_token(&runtime).await.expect("rotate");
        let output = runtime.stdout.lock().expect("stdout");
        assert!(output.contains("http://localhost:58627/#token="));
    }

    #[tokio::test]
    async fn color_mode_dims_labels_and_token_fragments() {
        let mut runtime = RuntimeMock::new();
        runtime.color = true;
        runtime.instance = Some(LiveServerInstance {
            host: "127.0.0.1".to_owned(),
            port: 58_627,
        });
        handle_rotate_token(&runtime).await.expect("rotate");
        let output = runtime.stdout.lock().expect("stdout");
        assert!(output.contains("\u{1b}[1mNew server token:\u{1b}[22m"));
        assert!(output.contains("\u{1b}[2mLocal:    \u{1b}[22m"));
        assert!(output.contains("\u{1b}[38;2;136;136;136m#token="));
    }

    #[tokio::test]
    async fn rotation_failure_stops_before_output_or_instance_lookup() {
        let mut runtime = RuntimeMock::new();
        runtime.fail_rotation = true;
        let error = handle_rotate_token(&runtime).await.expect_err("failure");
        assert_eq!(error.to_string(), "rotation failed");
        assert_eq!(runtime.calls.lock().expect("calls").as_slice(), ["rotate"]);
        assert!(runtime.stdout.lock().expect("stdout").is_empty());
    }
}
