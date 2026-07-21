use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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
