use serde::{Deserialize, Serialize};

use super::validation::{non_empty, optional_non_empty, optional_non_empty_vec, positive_u64};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCatalogItem {
    #[serde(deserialize_with = "non_empty")]
    pub provider: String,
    #[serde(deserialize_with = "non_empty")]
    pub model: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub display_name: Option<String>,
    #[serde(deserialize_with = "positive_u64")]
    pub max_context_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub support_efforts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderCatalogStatus {
    Connected,
    Error,
    Unconfigured,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCatalogItem {
    #[serde(deserialize_with = "non_empty")]
    pub id: String,
    #[serde(rename = "type", deserialize_with = "non_empty")]
    pub provider_type: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub base_url: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub default_model: Option<String>,
    pub has_api_key: bool,
    pub status: ProviderCatalogStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty_vec"
    )]
    pub models: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRefreshChange {
    #[serde(deserialize_with = "non_empty")]
    pub provider_id: String,
    #[serde(deserialize_with = "non_empty")]
    pub provider_name: String,
    pub added: u64,
    pub removed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRefreshFailure {
    #[serde(deserialize_with = "non_empty")]
    pub provider: String,
    #[serde(deserialize_with = "non_empty")]
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{file::FileMeta, workspace::Workspace};

    #[test]
    fn simple_domain_schemas_validate_and_preserve_wire_names() {
        let workspace: Workspace = serde_json::from_value(serde_json::json!({
            "id":"wd_demo_012345abcdef","root":"/repo","name":"Demo",
            "created_at":"2026-06-04T18:30:00+08:00",
            "last_opened_at":"2026-06-04T10:30:00Z","session_count":2
        }))
        .unwrap();
        assert_eq!(workspace.created_at, "2026-06-04T10:30:00.000Z");
        assert!(
            serde_json::from_value::<Workspace>(serde_json::json!({
                "id":"bad","root":"/repo","name":"Demo",
                "created_at":"2026-06-04T10:30:00Z",
                "last_opened_at":"2026-06-04T10:30:00Z","session_count":0
            }))
            .is_err()
        );

        let provider: ProviderCatalogItem = serde_json::from_value(serde_json::json!({
            "id":"openai","type":"openai","has_api_key":true,
            "status":"connected","models":["gpt-4.1"]
        }))
        .unwrap();
        assert_eq!(serde_json::to_value(provider).unwrap()["type"], "openai");
        assert!(
            serde_json::from_value::<ProviderCatalogItem>(serde_json::json!({
                "id":"openai","type":"openai","base_url":null,"has_api_key":true,
                "status":"connected"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ModelCatalogItem>(serde_json::json!({
                "provider":"p","model":"m","max_context_size":0
            }))
            .is_err()
        );

        let file: FileMeta = serde_json::from_value(serde_json::json!({
            "id":"f","name":"a.txt","media_type":"text/plain","size":0,
            "created_at":"2026-06-04T10:30:00Z"
        }))
        .unwrap();
        assert_eq!(file.created_at, "2026-06-04T10:30:00.000Z");
        assert!(
            serde_json::from_value::<FileMeta>(serde_json::json!({
                "id":"f","name":"a.txt","media_type":"text/plain","size":0,
                "created_at":"2026-06-04T10:30:00Z","expires_at":null
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<crate::protocol::WorkspaceCreate>(serde_json::json!({
                "root":"/repo","name":null
            }))
            .is_err()
        );
    }
}
