//! Host child-process spawning contract and structured errors.
//!
//! Original: `packages/agent-core-v2/src/os/interface/hostProcess.ts`.

use std::{
    collections::HashMap,
    error::Error,
    fmt,
    ops::Deref,
    sync::{Arc, LazyLock},
};

use async_trait::async_trait;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::Mutex,
};

use crate::_base::{
    di::instantiation::ServiceIdentifier,
    errors::{
        codes::{ErrorDomain, ErrorInfo, register_error_domain},
        errors::{Error2, Error2Options},
    },
};

pub const OS_PROCESS_SPAWN_FAILED: &str = "os.process.spawn_failed";
pub const OS_PROCESS_KILL_FAILED: &str = "os.process.kill_failed";

pub static OS_PROCESS_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("OS_PROCESS_SPAWN_FAILED", OS_PROCESS_SPAWN_FAILED),
        ("OS_PROCESS_KILL_FAILED", OS_PROCESS_KILL_FAILED),
    ],
    retryable: &[],
    info: &[
        (
            OS_PROCESS_SPAWN_FAILED,
            ErrorInfo {
                title: "Failed to spawn process",
                retryable: false,
                public: true,
                action: Some("Check that the command exists and is executable."),
            },
        ),
        (
            OS_PROCESS_KILL_FAILED,
            ErrorInfo {
                title: "Failed to kill process",
                retryable: false,
                public: true,
                action: None,
            },
        ),
    ],
};

static OS_PROCESS_ERRORS_REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&OS_PROCESS_ERRORS).expect("host process error codes are unique");
});

pub fn ensure_os_process_errors_registered() {
    LazyLock::force(&OS_PROCESS_ERRORS_REGISTERED);
}

#[derive(Clone, Debug)]
pub struct HostProcessError {
    inner: Box<Error2>,
}

impl HostProcessError {
    pub fn with_options(
        code: &'static str,
        message: impl Into<String>,
        mut options: Error2Options,
    ) -> Self {
        ensure_os_process_errors_registered();
        options
            .name
            .get_or_insert_with(|| "HostProcessError".into());
        Self {
            inner: Box::new(Error2::with_options(code, message, options)),
        }
    }

    pub fn code(&self) -> &str {
        &self.inner.code
    }
    pub fn error(&self) -> &Error2 {
        &self.inner
    }
}

impl fmt::Display for HostProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(formatter)
    }
}

impl Error for HostProcessError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.inner.source()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProcessShell {
    Default,
    Command(String),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HostProcessOptions {
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub shell: Option<ProcessShell>,
    pub detached: Option<bool>,
    pub windows_hide: Option<bool>,
    pub merge_stderr: Option<bool>,
    pub timeout_millis: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessSignal {
    Terminate,
    Kill,
    Interrupt,
}

pub type ProcessReader = Box<dyn AsyncRead + Send + Unpin>;
pub type ProcessWriter = Box<dyn AsyncWrite + Send + Unpin>;
pub type SharedProcessReader = Arc<Mutex<ProcessReader>>;
pub type SharedProcessWriter = Arc<Mutex<ProcessWriter>>;

#[async_trait]
pub trait HostProcess: Send + Sync {
    fn pid(&self) -> i64;
    fn exit_code(&self) -> Option<i32>;
    fn stdin(&self) -> SharedProcessWriter;
    fn stdout(&self) -> SharedProcessReader;
    fn stderr(&self) -> SharedProcessReader;
    async fn wait(&self) -> Result<i32, HostProcessError>;
    async fn kill(&self, signal: Option<ProcessSignal>) -> Result<(), HostProcessError>;
    fn dispose(&self);
}

#[async_trait]
pub trait HostProcessService: Send + Sync {
    async fn spawn(
        &self,
        command: &str,
        args: &[String],
        options: HostProcessOptions,
    ) -> Result<Arc<dyn HostProcess>, HostProcessError>;
}

#[derive(Clone)]
pub struct HostProcessServiceHandle(pub Arc<dyn HostProcessService>);

impl Deref for HostProcessServiceHandle {
    type Target = dyn HostProcessService;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const HOST_PROCESS_SERVICE_ID: ServiceIdentifier<HostProcessServiceHandle> =
    ServiceIdentifier::new("hostProcessService");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::errors::codes::error_info;

    #[test]
    fn error_domain_and_service_identity_match_source() {
        ensure_os_process_errors_registered();
        assert_eq!(HOST_PROCESS_SERVICE_ID.to_string(), "hostProcessService");
        assert_eq!(
            error_info(OS_PROCESS_SPAWN_FAILED).action.as_deref(),
            Some("Check that the command exists and is executable.")
        );
        assert!(!error_info(OS_PROCESS_KILL_FAILED).retryable);
        let error = HostProcessError::with_options(
            OS_PROCESS_SPAWN_FAILED,
            "failed",
            Error2Options::default(),
        );
        assert_eq!(error.code(), OS_PROCESS_SPAWN_FAILED);
        assert_eq!(error.error().name, "HostProcessError");
    }
}
