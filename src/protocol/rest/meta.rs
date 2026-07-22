use serde::{Deserialize, Deserializer, Serialize};

use crate::protocol::rest::fs::FsOpenInAppId;
use crate::protocol::time::IsoDateTime;
use crate::protocol::validation::non_empty;

fn deserialize_true<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    match bool::deserialize(deserializer)? {
        true => Ok(true),
        false => Err(serde::de::Error::custom("must be true")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaCapabilities {
    #[serde(deserialize_with = "deserialize_true")]
    pub websocket: bool,
    #[serde(deserialize_with = "deserialize_true")]
    pub file_upload: bool,
    #[serde(deserialize_with = "deserialize_true")]
    pub fs_query: bool,
    #[serde(deserialize_with = "deserialize_true")]
    pub mcp: bool,
    #[serde(deserialize_with = "deserialize_true")]
    pub tasks: bool,
    #[serde(deserialize_with = "deserialize_true")]
    pub terminal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendGeneration {
    V1,
    V2,
}

// Original: rest/meta.ts, metaResponseSchema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaResponse {
    #[serde(deserialize_with = "non_empty")]
    pub server_version: String,
    pub capabilities: MetaCapabilities,
    #[serde(deserialize_with = "non_empty")]
    pub server_id: String,
    pub started_at: IsoDateTime,
    pub open_in_apps: Vec<FsOpenInAppId>,
    pub dangerous_bypass_auth: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<BackendGeneration>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::rest::{AuthSummary, Connection, GuiStoreGetItemResponse};

    #[test]
    fn low_dependency_rest_schemas_enforce_literals_and_required_nulls() {
        let meta: MetaResponse = serde_json::from_value(serde_json::json!({
            "server_version":"1.0","capabilities":{"websocket":true,"file_upload":true,
                "fs_query":true,"mcp":true,"tasks":true,"terminal":true},
            "server_id":"srv","started_at":"2026-06-04T18:30:00+08:00",
            "open_in_apps":["vscode"],"dangerous_bypass_auth":false,"backend":"v2"
        }))
        .unwrap();
        assert_eq!(meta.started_at, "2026-06-04T10:30:00.000Z");
        assert!(
            serde_json::from_value::<MetaResponse>(serde_json::json!({
                "server_version":"1.0","capabilities":{"websocket":false,"file_upload":true,
                    "fs_query":true,"mcp":true,"tasks":true,"terminal":true},
                "server_id":"srv","started_at":"2026-06-04T10:30:00Z",
                "open_in_apps":[],"dangerous_bypass_auth":false
            }))
            .is_err()
        );

        assert!(
            serde_json::from_value::<AuthSummary>(serde_json::json!({
                "ready":false,"providers_count":0,"managed_provider":null
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<Connection>(serde_json::json!({
                "id":"c","connected_at":"2026-06-04T10:30:00Z","remote_address":null,
                "user_agent":null,"has_client_hello":false,"subscriptions":[]
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<GuiStoreGetItemResponse>(serde_json::json!({
                "value":null
            }))
            .is_ok()
        );
    }
}
