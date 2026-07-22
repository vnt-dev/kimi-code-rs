use serde::{Deserialize, Deserializer, Serialize};

use crate::protocol::validation::required_nullable;

fn deserialize_key<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let key = String::deserialize(deserializer)?;
    let length = key.encode_utf16().count();
    if (1..=256).contains(&length) {
        Ok(key)
    } else {
        Err(serde::de::Error::custom(
            "key must contain between 1 and 256 characters",
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuiStoreGetItemQuery {
    #[serde(deserialize_with = "deserialize_key")]
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuiStoreSetItemBody {
    #[serde(deserialize_with = "deserialize_key")]
    pub key: String,
    pub value: String,
}

pub type GuiStoreRemoveItemBody = GuiStoreGetItemQuery;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuiStoreGetItemResponse {
    #[serde(deserialize_with = "required_nullable")]
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuiStoreLengthResponse {
    pub length: f64,
}
