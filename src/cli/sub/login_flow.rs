use std::{
    error::Error,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;

use crate::cli::version::{CLI_USER_AGENT_PRODUCT, HostIdentity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginUiMode {
    Cli,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginHarnessOptions {
    pub identity: HostIdentity,
    pub ui_mode: LoginUiMode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceCode {
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub user_code: String,
    pub expires_in: Option<f64>,
}

impl DeviceCode {
    fn browser_url(&self) -> &str {
        if self.verification_uri_complete.is_empty() {
            &self.verification_uri
        } else {
            &self.verification_uri_complete
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginResult {
    pub provider_name: String,
}

#[derive(Debug, Clone, Default)]
pub struct LoginCancellation {
    aborted: Arc<AtomicBool>,
}

impl LoginCancellation {
    pub fn abort(&self) {
        self.aborted.store(true, Ordering::SeqCst);
    }

    pub fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginDisposition {
    Exit(i32),
}

#[derive(Debug)]
pub struct LoginRuntimeError(Box<dyn Error + Send + Sync>);

impl LoginRuntimeError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for LoginRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for LoginRuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

pub trait LoginHarness: Send {}

#[async_trait]
pub trait LoginRuntime: Send + Sync {
    fn create_harness(
        &self,
        options: LoginHarnessOptions,
    ) -> Result<Box<dyn LoginHarness>, LoginRuntimeError>;

    fn register_sigint_abort(&self, cancellation: LoginCancellation);

    async fn login(
        &self,
        harness: Box<dyn LoginHarness>,
        cancellation: LoginCancellation,
        on_device_code: &mut (dyn FnMut(DeviceCode) + Send),
    ) -> Result<LoginResult, LoginRuntimeError>;

    fn open_url(&self, url: &str) -> Result<(), LoginRuntimeError>;

    fn version(&self) -> &str;

    fn write_stderr(&self, text: &str);
}

// Original:
//   apps/kimi-code/src/cli/sub/login-flow.ts
//   runLoginFlow()
//
// Rust adaptation:
//   Process exit is returned to the binary entrypoint, while an atomic token
//   carries AbortController's observable cancellation state into the injected
//   asynchronous login operation.
pub async fn run_login_flow(
    runtime: &dyn LoginRuntime,
) -> Result<LoginDisposition, LoginRuntimeError> {
    let version = runtime.version().to_owned();
    let harness = runtime.create_harness(LoginHarnessOptions {
        identity: HostIdentity {
            user_agent_product: CLI_USER_AGENT_PRODUCT.to_owned(),
            version,
        },
        ui_mode: LoginUiMode::Cli,
    })?;
    let cancellation = LoginCancellation::default();
    runtime.register_sigint_abort(cancellation.clone());

    let mut on_device_code = |data: DeviceCode| {
        let url = data.browser_url();
        let mut lines = vec![
            String::new(),
            format!("Opening browser for Kimi device login: {url}"),
            format!(
                "If the browser did not open, paste the URL above and enter code: {}",
                data.user_code
            ),
        ];
        if let Some(expires_in) = data.expires_in {
            lines.push(format!("Code expires in {expires_in}s."));
        }
        lines.push("Waiting for authorization to complete...".to_owned());
        lines.push(String::new());
        runtime.write_stderr(&lines.join("\n"));
        let _ = runtime.open_url(url);
    };

    match runtime
        .login(harness, cancellation.clone(), &mut on_device_code)
        .await
    {
        Ok(result) => {
            runtime.write_stderr(&format!("Logged in to {}.\n", result.provider_name));
            Ok(LoginDisposition::Exit(0))
        }
        Err(error) => {
            if cancellation.is_aborted() {
                runtime.write_stderr("Login cancelled.\n");
            } else {
                runtime.write_stderr(&format!("Login failed: {error}\n"));
            }
            Ok(LoginDisposition::Exit(1))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Debug, Clone, Copy)]
    struct TestError(&'static str);

    impl fmt::Display for TestError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.0)
        }
    }

    impl Error for TestError {}

    struct HarnessMock;
    impl LoginHarness for HarnessMock {}

    struct RuntimeMock {
        harness_options: Mutex<Vec<LoginHarnessOptions>>,
        registered: Mutex<Vec<LoginCancellation>>,
        device_code: Option<DeviceCode>,
        login_error: bool,
        cancel_during_login: bool,
        open_error: bool,
        events: Mutex<Vec<String>>,
        stderr: Mutex<String>,
    }

    impl RuntimeMock {
        fn success() -> Self {
            Self {
                harness_options: Mutex::new(Vec::new()),
                registered: Mutex::new(Vec::new()),
                device_code: None,
                login_error: false,
                cancel_during_login: false,
                open_error: false,
                events: Mutex::new(Vec::new()),
                stderr: Mutex::new(String::new()),
            }
        }
    }

    #[async_trait]
    impl LoginRuntime for RuntimeMock {
        fn create_harness(
            &self,
            options: LoginHarnessOptions,
        ) -> Result<Box<dyn LoginHarness>, LoginRuntimeError> {
            self.harness_options
                .lock()
                .expect("harness options")
                .push(options);
            Ok(Box::new(HarnessMock))
        }

        fn register_sigint_abort(&self, cancellation: LoginCancellation) {
            self.registered
                .lock()
                .expect("registered")
                .push(cancellation);
        }

        async fn login(
            &self,
            _: Box<dyn LoginHarness>,
            cancellation: LoginCancellation,
            on_device_code: &mut (dyn FnMut(DeviceCode) + Send),
        ) -> Result<LoginResult, LoginRuntimeError> {
            if let Some(device_code) = self.device_code.clone() {
                on_device_code(device_code);
            }
            if self.cancel_during_login {
                cancellation.abort();
            }
            if self.login_error {
                Err(LoginRuntimeError::new(TestError("authorization denied")))
            } else {
                Ok(LoginResult {
                    provider_name: "kimi-code".to_owned(),
                })
            }
        }

        fn open_url(&self, url: &str) -> Result<(), LoginRuntimeError> {
            self.events
                .lock()
                .expect("events")
                .push(format!("open:{url}"));
            if self.open_error {
                Err(LoginRuntimeError::new(TestError("browser unavailable")))
            } else {
                Ok(())
            }
        }

        fn version(&self) -> &str {
            "1.2.3-test"
        }

        fn write_stderr(&self, text: &str) {
            self.events
                .lock()
                .expect("events")
                .push(format!("stderr:{text}"));
            self.stderr.lock().expect("stderr").push_str(text);
        }
    }

    fn device_code(expires_in: Option<f64>) -> DeviceCode {
        DeviceCode {
            verification_uri: "https://auth.example/device".to_owned(),
            verification_uri_complete: "https://auth.example/device?code=ABCD".to_owned(),
            user_code: "ABCD".to_owned(),
            expires_in,
        }
    }

    #[tokio::test]
    async fn creates_cli_harness_registers_sigint_and_reports_success() {
        let runtime = RuntimeMock::success();

        let disposition = run_login_flow(&runtime).await.expect("login");

        assert_eq!(disposition, LoginDisposition::Exit(0));
        assert_eq!(
            runtime.harness_options.lock().expect("harness").as_slice(),
            [LoginHarnessOptions {
                identity: HostIdentity {
                    user_agent_product: "kimi-code-cli".to_owned(),
                    version: "1.2.3-test".to_owned(),
                },
                ui_mode: LoginUiMode::Cli,
            }]
        );
        assert_eq!(runtime.registered.lock().expect("registered").len(), 1);
        assert_eq!(
            runtime.stderr.lock().expect("stderr").as_str(),
            "Logged in to kimi-code.\n"
        );
    }

    #[tokio::test]
    async fn prints_manual_fallback_before_best_effort_browser_open() {
        let mut runtime = RuntimeMock::success();
        runtime.device_code = Some(device_code(Some(900.0)));
        runtime.open_error = true;

        let disposition = run_login_flow(&runtime).await.expect("login");

        assert_eq!(disposition, LoginDisposition::Exit(0));
        let events = runtime.events.lock().expect("events");
        assert!(events[0].starts_with("stderr:\nOpening browser"));
        assert_eq!(
            events[1],
            "open:https://auth.example/device?code=ABCD".to_owned()
        );
        assert!(events[0].contains("enter code: ABCD"));
        assert!(events[0].contains("Code expires in 900s."));
        assert!(events[0].ends_with("Waiting for authorization to complete...\n"));
        assert_eq!(events[2], "stderr:Logged in to kimi-code.\n");
    }

    #[tokio::test]
    async fn falls_back_to_verification_uri_and_omits_missing_expiry() {
        let mut runtime = RuntimeMock::success();
        let mut data = device_code(None);
        data.verification_uri_complete.clear();
        runtime.device_code = Some(data);

        run_login_flow(&runtime).await.expect("login");

        let events = runtime.events.lock().expect("events");
        assert_eq!(events[1], "open:https://auth.example/device".to_owned());
        assert!(!events[0].contains("Code expires"));
    }

    #[tokio::test]
    async fn reports_login_failure_and_exits_one() {
        let mut runtime = RuntimeMock::success();
        runtime.login_error = true;

        let disposition = run_login_flow(&runtime).await.expect("handled failure");

        assert_eq!(disposition, LoginDisposition::Exit(1));
        assert_eq!(
            runtime.stderr.lock().expect("stderr").as_str(),
            "Login failed: authorization denied\n"
        );
    }

    #[tokio::test]
    async fn cancellation_takes_precedence_over_login_error_text() {
        let mut runtime = RuntimeMock::success();
        runtime.login_error = true;
        runtime.cancel_during_login = true;

        let disposition = run_login_flow(&runtime).await.expect("cancelled");

        assert_eq!(disposition, LoginDisposition::Exit(1));
        assert_eq!(
            runtime.stderr.lock().expect("stderr").as_str(),
            "Login cancelled.\n"
        );
    }
}
