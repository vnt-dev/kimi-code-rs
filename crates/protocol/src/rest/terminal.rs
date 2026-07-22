use serde::{Deserialize, Deserializer, Serialize};

use crate::time::IsoDateTime;
use crate::validation::{
    OptionalNullable, literal_true, non_empty, optional_non_null, positive_u64,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalStatus {
    Running,
    Exited,
}

// Original: rest/terminal.ts, terminalSchema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Terminal {
    #[serde(deserialize_with = "non_empty")]
    pub id: String,
    #[serde(deserialize_with = "non_empty")]
    pub session_id: String,
    #[serde(deserialize_with = "non_empty")]
    pub cwd: String,
    #[serde(deserialize_with = "non_empty")]
    pub shell: String,
    #[serde(deserialize_with = "positive_u64")]
    pub cols: u64,
    #[serde(deserialize_with = "positive_u64")]
    pub rows: u64,
    pub status: TerminalStatus,
    pub created_at: IsoDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub exited_at: Option<IsoDateTime>,
    #[serde(default, skip_serializing_if = "OptionalNullable::is_absent")]
    pub exit_code: OptionalNullable<i64>,
}

// Original: rest/terminal.ts, createTerminalRequestSchema and isAbsolutePath().
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateTerminalRequest {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_relative_cwd"
    )]
    pub cwd: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty_string"
    )]
    pub shell: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_positive_u64"
    )]
    pub cols: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_positive_u64"
    )]
    pub rows: Option<u64>,
}

fn optional_relative_cwd<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        return Err(serde::de::Error::custom("cwd must not be empty"));
    }
    let bytes = value.as_bytes();
    let windows_absolute = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\');
    if value.starts_with(['/', '\\']) || windows_absolute {
        Err(serde::de::Error::custom(
            "cwd must be relative to the session workspace",
        ))
    } else {
        Ok(Some(value))
    }
}

fn optional_non_empty_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    non_empty(deserializer).map(Some)
}

fn optional_positive_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    positive_u64(deserializer).map(Some)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListTerminalsResponse {
    pub items: Vec<Terminal>,
}

pub type GetTerminalResponse = Terminal;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseTerminalResponse {
    #[serde(deserialize_with = "literal_true")]
    pub closed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_terminal_resources_and_create_paths() {
        let terminal: Terminal = serde_json::from_value(serde_json::json!({
            "id": "term_01HX",
            "session_id": "sess_01",
            "cwd": "/tmp/example",
            "shell": "/bin/zsh",
            "cols": 120,
            "rows": 32,
            "status": "exited",
            "created_at": "2026-06-04T10:30:00Z",
            "exit_code": null
        }))
        .unwrap();
        assert_eq!(terminal.exit_code, OptionalNullable::Null);

        let relative: CreateTerminalRequest = serde_json::from_value(serde_json::json!({
            "cwd": "packages/server",
            "cols": 100
        }))
        .unwrap();
        assert_eq!(relative.cwd.as_deref(), Some("packages/server"));

        for cwd in ["/tmp/outside", "\\\\server", "C:\\outside"] {
            assert!(
                serde_json::from_value::<CreateTerminalRequest>(serde_json::json!({"cwd": cwd}))
                    .is_err()
            );
        }
        assert!(
            serde_json::from_value::<CloseTerminalResponse>(serde_json::json!({"closed": false}))
                .is_err()
        );
    }
}
