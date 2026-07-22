use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::sync::LazyLock;

use super::time::IsoDateTime;
use super::validation::non_empty;

static WORKSPACE_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^wd_[a-z0-9._-]+_[0-9a-f]{12}$").expect("static workspace ID regex must compile")
});

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct WorkspaceId(String);

impl WorkspaceId {
    pub fn parse(value: impl Into<String>) -> Result<Self, WorkspaceIdError> {
        let value = value.into();
        if WORKSPACE_ID_RE.is_match(&value) {
            Ok(Self(value))
        } else {
            Err(WorkspaceIdError)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for WorkspaceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceIdError;

impl fmt::Display for WorkspaceIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("workspace_id must be a wd_<slug>_<hash12> string")
    }
}

impl std::error::Error for WorkspaceIdError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    #[serde(deserialize_with = "non_empty")]
    pub root: String,
    #[serde(deserialize_with = "deserialize_workspace_name")]
    pub name: String,
    pub created_at: IsoDateTime,
    pub last_opened_at: IsoDateTime,
    pub session_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCreate {
    #[serde(deserialize_with = "non_empty")]
    pub root: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_workspace_name"
    )]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceUpdate {
    #[serde(deserialize_with = "deserialize_workspace_name")]
    pub name: String,
}

fn validate_name<E: serde::de::Error>(value: String) -> Result<String, E> {
    // JavaScript/Zod string lengths count UTF-16 code units.
    let length = value.encode_utf16().count();
    if length == 0 || length > 100 {
        Err(E::custom("name must contain between 1 and 100 characters"))
    } else {
        Ok(value)
    }
}

fn deserialize_workspace_name<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    validate_name(String::deserialize(deserializer)?)
}

fn deserialize_optional_workspace_name<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?
        .map(validate_name)
        .transpose()
}
