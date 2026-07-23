//! Interactive terminal wire types and host PTY contract.
//!
//! Original: `packages/agent-core-v2/src/os/interface/terminal.ts` and
//! `terminalErrors.ts`.

use std::{
    error::Error,
    fmt,
    path::Path,
    sync::{Arc, LazyLock},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::_base::{
    di::instantiation::ServiceIdentifier,
    errors::codes::{ErrorDomain, ErrorInfo, register_error_domain},
    event::Event,
    utils::iso_date_time::IsoDateTime,
};

pub const TERMINAL_NOT_FOUND: &str = "terminal.not_found";

pub static TERMINAL_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[("TERMINAL_NOT_FOUND", TERMINAL_NOT_FOUND)],
    retryable: &[],
    info: &[(
        TERMINAL_NOT_FOUND,
        ErrorInfo {
            title: "Terminal not found",
            retryable: false,
            public: true,
            action: None,
        },
    )],
};

static TERMINAL_ERRORS_REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&TERMINAL_ERRORS).expect("terminal error codes are unique");
});

pub fn ensure_terminal_errors_registered() {
    LazyLock::force(&TERMINAL_ERRORS_REGISTERED);
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    Running,
    Exited,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "RawTerminal")]
pub struct Terminal {
    pub id: String,
    pub session_id: String,
    pub cwd: String,
    pub shell: String,
    pub cols: u32,
    pub rows: u32,
    pub status: TerminalStatus,
    pub created_at: IsoDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exited_at: Option<IsoDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<Value>,
}

impl Terminal {
    pub fn validate(&self) -> Result<(), TerminalValidationError> {
        require_nonempty(&self.id, "id")?;
        require_nonempty(&self.session_id, "session_id")?;
        require_nonempty(&self.cwd, "cwd")?;
        require_nonempty(&self.shell, "shell")?;
        require_positive(self.cols, "cols")?;
        require_positive(self.rows, "rows")?;
        if let Some(value) = &self.exit_code
            && !value.is_null()
            && value.as_i64().is_none()
        {
            return Err(TerminalValidationError(
                "exit_code must be an integer or null",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct RawTerminal {
    id: String,
    session_id: String,
    cwd: String,
    shell: String,
    cols: u32,
    rows: u32,
    status: TerminalStatus,
    created_at: IsoDateTime,
    exited_at: Option<IsoDateTime>,
    exit_code: Option<Value>,
}

impl TryFrom<RawTerminal> for Terminal {
    type Error = TerminalValidationError;

    fn try_from(raw: RawTerminal) -> Result<Self, Self::Error> {
        let terminal = Self {
            id: raw.id,
            session_id: raw.session_id,
            cwd: raw.cwd,
            shell: raw.shell,
            cols: raw.cols,
            rows: raw.rows,
            status: raw.status,
            created_at: raw.created_at,
            exited_at: raw.exited_at,
            exit_code: raw.exit_code,
        };
        terminal.validate()?;
        Ok(terminal)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "RawCreateTerminalRequest")]
pub struct CreateTerminalRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cols: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<u32>,
}

#[derive(Deserialize)]
struct RawCreateTerminalRequest {
    cwd: Option<String>,
    shell: Option<String>,
    cols: Option<u32>,
    rows: Option<u32>,
}

impl TryFrom<RawCreateTerminalRequest> for CreateTerminalRequest {
    type Error = TerminalValidationError;

    fn try_from(raw: RawCreateTerminalRequest) -> Result<Self, Self::Error> {
        let request = Self {
            cwd: raw.cwd,
            shell: raw.shell,
            cols: raw.cols,
            rows: raw.rows,
        };
        request.validate()?;
        Ok(request)
    }
}

impl CreateTerminalRequest {
    pub fn validate(&self) -> Result<(), TerminalValidationError> {
        if let Some(cwd) = &self.cwd {
            require_nonempty(cwd, "cwd")?;
            if is_absolute_path(cwd) {
                return Err(TerminalValidationError(
                    "cwd must be relative to the session workspace",
                ));
            }
        }
        if let Some(shell) = &self.shell {
            require_nonempty(shell, "shell")?;
        }
        if let Some(cols) = self.cols {
            require_positive(cols, "cols")?;
        }
        if let Some(rows) = self.rows {
            require_positive(rows, "rows")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalOutputPayload {
    pub data: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalExitPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum TerminalFrame {
    #[serde(rename = "terminal_output")]
    Output {
        seq: u64,
        session_id: String,
        terminal_id: String,
        timestamp: IsoDateTime,
        payload: TerminalOutputPayload,
    },
    #[serde(rename = "terminal_exit")]
    Exit {
        session_id: String,
        terminal_id: String,
        timestamp: IsoDateTime,
        payload: TerminalExitPayload,
    },
}

pub trait TerminalAttachSink: Send + Sync {
    fn id(&self) -> &str;
    fn send(&self, frame: TerminalFrame);
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TerminalAttachOptions {
    pub since_seq: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalSpawnOptions {
    pub cwd: String,
    pub shell: String,
    pub cols: u32,
    pub rows: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalProcessExit {
    pub exit_code: Option<i32>,
}

#[derive(Debug, thiserror::Error)]
#[error("terminal process operation failed: {0}")]
pub struct TerminalProcessError(pub String);

pub trait TerminalProcess: Send + Sync {
    fn on_process_data(&self) -> Event<String>;
    fn on_process_exit(&self) -> Event<TerminalProcessExit>;
    fn write(&self, data: &str) -> Result<(), TerminalProcessError>;
    fn resize(&self, cols: u32, rows: u32) -> Result<(), TerminalProcessError>;
    fn kill(&self) -> Result<(), TerminalProcessError>;
}

#[async_trait]
pub trait HostTerminalService: Send + Sync {
    async fn spawn(
        &self,
        options: TerminalSpawnOptions,
    ) -> Result<Arc<dyn TerminalProcess>, TerminalProcessError>;
}

pub const HOST_TERMINAL_SERVICE_ID: ServiceIdentifier<dyn HostTerminalService> =
    ServiceIdentifier::new("hostTerminalService");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalValidationError(&'static str);

impl fmt::Display for TerminalValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for TerminalValidationError {}

fn require_nonempty(value: &str, field: &'static str) -> Result<(), TerminalValidationError> {
    if value.is_empty() {
        Err(TerminalValidationError(match field {
            "id" => "id must not be empty",
            "session_id" => "session_id must not be empty",
            "cwd" => "cwd must not be empty",
            _ => "shell must not be empty",
        }))
    } else {
        Ok(())
    }
}

fn require_positive(value: u32, field: &'static str) -> Result<(), TerminalValidationError> {
    if value == 0 {
        Err(TerminalValidationError(if field == "cols" {
            "cols must be positive"
        } else {
            "rows must be positive"
        }))
    } else {
        Ok(())
    }
}

fn is_absolute_path(value: &str) -> bool {
    Path::new(value).is_absolute()
        || value.starts_with(['/', '\\'])
        || (value.len() >= 3
            && value.as_bytes()[0].is_ascii_alphabetic()
            && value.as_bytes()[1] == b':'
            && matches!(value.as_bytes()[2], b'/' | b'\\'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::errors::codes::error_info;

    #[test]
    fn request_validation_rejects_absolute_empty_and_nonpositive_values() {
        for cwd in ["/tmp", "\\server", "C:\\work"] {
            assert!(
                CreateTerminalRequest {
                    cwd: Some(cwd.into()),
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            CreateTerminalRequest {
                cwd: Some("src".into()),
                cols: Some(80),
                rows: Some(24),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
        assert!(
            CreateTerminalRequest {
                cols: Some(0),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            serde_json::from_value::<CreateTerminalRequest>(serde_json::json!({"cwd": "/tmp"}))
                .is_err()
        );
    }

    #[test]
    fn frames_and_status_preserve_external_names() {
        assert_eq!(
            serde_json::to_value(TerminalStatus::Running).unwrap(),
            "running"
        );
        let frame = TerminalFrame::Output {
            seq: 1,
            session_id: "s".into(),
            terminal_id: "t".into(),
            timestamp: kimi_code_protocol::time::parse_iso_date_time("2025-01-01T00:00:00Z")
                .unwrap(),
            payload: TerminalOutputPayload { data: "ok".into() },
        };
        assert_eq!(
            serde_json::to_value(frame).unwrap()["type"],
            "terminal_output"
        );
    }

    #[test]
    fn error_and_service_identifiers_are_registered() {
        ensure_terminal_errors_registered();
        assert_eq!(HOST_TERMINAL_SERVICE_ID.to_string(), "hostTerminalService");
        assert_eq!(error_info(TERMINAL_NOT_FOUND).title, "Terminal not found");
    }
}
