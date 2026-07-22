use serde::{Deserialize, Serialize};

use super::time::IsoDateTime;
use super::validation::non_empty;

// Original: file.ts, fileMetaSchema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMeta {
    #[serde(deserialize_with = "non_empty")]
    pub id: String,
    #[serde(deserialize_with = "non_empty")]
    pub name: String,
    #[serde(deserialize_with = "non_empty")]
    pub media_type: String,
    pub size: u64,
    pub created_at: IsoDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_time"
    )]
    pub expires_at: Option<IsoDateTime>,
}

fn deserialize_optional_time<'de, D>(deserializer: D) -> Result<Option<IsoDateTime>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    IsoDateTime::deserialize(deserializer).map(Some)
}
