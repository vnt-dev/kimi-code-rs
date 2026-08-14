//! Session-global transcript attachment entities.
//!
//! Original: `packages/transcript/src/model/attachment.ts`.

use serde::{Deserialize, Serialize};

use super::AttachmentId;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum AttachmentSource {
    #[serde(rename = "url")]
    Url { url: String },
    #[serde(rename = "file", rename_all = "camelCase")]
    File { file_id: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptAttachment {
    pub attachment_id: AttachmentId,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_utils::lenient_u64::deserialize"
    )]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<AttachmentSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}
