use serde::{Deserialize, Deserializer, Serialize};

use crate::validation::{literal_true, non_empty};

// Original: rest/fsBrowse.ts, fsBrowseQuerySchema.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsBrowseQuery {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub path: Option<String>,
}

fn optional_non_empty<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    non_empty(deserializer).map(Some)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsBrowseEntry {
    #[serde(deserialize_with = "non_empty")]
    pub name: String,
    #[serde(deserialize_with = "non_empty")]
    pub path: String,
    #[serde(deserialize_with = "literal_true")]
    pub is_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsBrowseResponse {
    #[serde(deserialize_with = "non_empty")]
    pub path: String,
    #[serde(deserialize_with = "nullable_non_empty")]
    pub parent: Option<String>,
    pub entries: Vec<FsBrowseEntry>,
}

fn nullable_non_empty<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    if value.as_ref().is_some_and(String::is_empty) {
        Err(serde::de::Error::custom("parent must not be empty"))
    } else {
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsHomeResponse {
    #[serde(deserialize_with = "non_empty")]
    pub home: String,
    #[serde(deserialize_with = "non_empty_strings")]
    pub recent_roots: Vec<String>,
}

fn non_empty_strings<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Vec::<String>::deserialize(deserializer)?;
    if values.iter().any(String::is_empty) {
        Err(serde::de::Error::custom("paths must not be empty"))
    } else {
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_browse_query_entries_and_nullable_parent() {
        assert!(serde_json::from_value::<FsBrowseQuery>(serde_json::json!({})).is_ok());
        assert!(serde_json::from_value::<FsBrowseQuery>(serde_json::json!({"path": ""})).is_err());
        assert!(
            serde_json::from_value::<FsBrowseEntry>(serde_json::json!({
                "name": "README.md", "path": "/code/README.md", "is_dir": false
            }))
            .is_err()
        );

        let root: FsBrowseResponse = serde_json::from_value(serde_json::json!({
            "path": "/", "parent": null, "entries": []
        }))
        .unwrap();
        assert_eq!(root.parent, None);
        assert!(
            serde_json::from_value::<FsBrowseResponse>(serde_json::json!({
                "path": "/", "entries": []
            }))
            .is_err()
        );
    }
}
