use serde::{Deserialize, Deserializer, Serialize};

use super::time::IsoDateTime;
use super::validation::positive_u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsKind {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsGitStatus {
    Clean,
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Ignored,
    Conflicted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FsEntry {
    pub path: String,
    pub name: String,
    pub kind: FsKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    pub modified_at: IsoDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_binary: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_symlink_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_status: Option<FsGitStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FsSearchHit {
    pub path: String,
    pub name: String,
    pub kind: FsKind,
    #[serde(deserialize_with = "deserialize_score")]
    pub score: f64,
    pub match_positions: Vec<u64>,
}

fn deserialize_score<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    let score = f64::deserialize(deserializer)?;
    if (0.0..=1.0).contains(&score) {
        Ok(score)
    } else {
        Err(serde::de::Error::custom("score must be between 0 and 1"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsGrepMatch {
    #[serde(deserialize_with = "positive_u64")]
    pub line: u64,
    #[serde(deserialize_with = "positive_u64")]
    pub col: u64,
    pub text: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsGrepFileHit {
    pub path: String,
    pub matches: Vec<FsGrepMatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsGitStatusEntry {
    pub path: String,
    pub status: FsGitStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rename_from: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsChangeKind {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsChangeAction {
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsChangeEntry {
    pub path: String,
    pub change: FsChangeAction,
    pub kind: FsChangeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_delta: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsChangeEvent {
    pub changes: Vec<FsChangeEntry>,
    #[serde(deserialize_with = "positive_u64")]
    pub coalesced_window_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, MessageContent, QuestionRequest};

    #[test]
    fn union_and_filesystem_schemas_enforce_wire_constraints() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "id":"msg_1","session_id":"sess_1","role":"assistant",
            "content":[
                {"type":"text","text":"hi"},
                {"type":"video","source":{"kind":"file","file_id":"file_1"}}
            ],
            "created_at":"2026-06-04T18:30:00+08:00"
        }))
        .unwrap();
        assert!(matches!(message.content[1], MessageContent::Video(_)));
        assert_eq!(
            serde_json::to_value(&message).unwrap()["content"][1]["type"],
            "video"
        );
        assert_eq!(message.created_at, "2026-06-04T10:30:00.000Z");

        let question = serde_json::json!({
            "question_id":"q","session_id":"s","questions":[{
                "id":"one","question":"Which?","options":[
                    {"id":"a","label":"A"},{"id":"b","label":"B"}
                ]
            }],"created_at":"2026-06-04T10:30:00Z"
        });
        assert!(serde_json::from_value::<QuestionRequest>(question).is_ok());
        assert!(
            serde_json::from_value::<QuestionRequest>(serde_json::json!({
                "question_id":"q","session_id":"s","questions":[],
                "created_at":"2026-06-04T10:30:00Z"
            }))
            .is_err()
        );

        assert!(
            serde_json::from_value::<FsSearchHit>(serde_json::json!({
                "path":"a","name":"a","kind":"file","score":1.1,"match_positions":[]
            }))
            .is_err()
        );
        let change: FsChangeEvent = serde_json::from_value(serde_json::json!({
            "changes":[{"path":"a","change":"modified","kind":"file","size_delta":-2}],
            "coalesced_window_ms":200
        }))
        .unwrap();
        assert_eq!(change.changes[0].size_delta, Some(-2));
    }
}
