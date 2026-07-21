use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Provider-defined thinking effort. Known values include `off` and `on`, but
/// providers may expose arbitrary named effort levels.
///
/// Original:
///   packages/kosong/src/provider.ts
///   ThinkingEffort
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThinkingEffort(String);

impl ThinkingEffort {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ThinkingEffort {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// Summary returned by the session-listing SDK surface.
///
/// Original:
///   packages/node-sdk/src/types.ts
///   SessionSummary
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub title: Option<String>,
    pub last_prompt: Option<String>,
    pub work_dir: String,
    pub session_dir: String,
    pub created_at: Option<f64>,
    pub updated_at: Option<f64>,
    pub archived: Option<bool>,
    pub metadata: Option<Map<String, Value>>,
    pub additional_dirs: Option<Vec<String>>,
}
