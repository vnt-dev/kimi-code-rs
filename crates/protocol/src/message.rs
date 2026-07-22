use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::time::IsoDateTime;
use super::validation::{non_empty, optional_non_empty};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ImageSource {
    Url {
        #[serde(deserialize_with = "non_empty")]
        url: String,
    },
    Base64 {
        #[serde(deserialize_with = "non_empty")]
        media_type: String,
        #[serde(deserialize_with = "non_empty")]
        data: String,
    },
    File {
        #[serde(deserialize_with = "non_empty")]
        file_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextContent {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolUseContent {
    #[serde(deserialize_with = "non_empty")]
    pub tool_call_id: String,
    #[serde(deserialize_with = "non_empty")]
    pub tool_name: String,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResultContent {
    #[serde(deserialize_with = "non_empty")]
    pub tool_call_id: String,
    pub output: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageContent {
    pub source: ImageSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoContent {
    pub source: ImageSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileContent {
    #[serde(deserialize_with = "non_empty")]
    pub file_id: String,
    pub name: String,
    #[serde(deserialize_with = "non_empty")]
    pub media_type: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingContent {
    pub thinking: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

// Original: message.ts, messageContentSchema discriminated union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageContent {
    Text(TextContent),
    ToolUse(ToolUseContent),
    ToolResult(ToolResultContent),
    Image(ImageContent),
    Video(VideoContent),
    File(FileContent),
    Thinking(ThinkingContent),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    #[serde(deserialize_with = "non_empty")]
    pub id: String,
    #[serde(deserialize_with = "non_empty")]
    pub session_id: String,
    pub role: MessageRole,
    pub content: Vec<MessageContent>,
    pub created_at: IsoDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub prompt_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub parent_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<IndexMap<String, Value>>,
}
