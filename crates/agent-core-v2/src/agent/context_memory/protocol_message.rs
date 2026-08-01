use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::_base::utils::iso_date_time::IsoDateTime;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

fn deserialize_non_empty_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        Err(serde::de::Error::custom(
            "string must contain at least 1 character",
        ))
    } else {
        Ok(value)
    }
}

fn deserialize_optional_non_empty_string<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?.map_or(Ok(None), |value| {
        if value.is_empty() {
            Err(serde::de::Error::custom(
                "string must contain at least 1 character",
            ))
        } else {
            Ok(Some(value))
        }
    })
}

fn deserialize_nonnegative_safe_integer<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    let number = value
        .as_f64()
        .filter(|number| {
            number.is_finite()
                && *number >= 0.0
                && number.fract() == 0.0
                && *number <= MAX_SAFE_INTEGER as f64
        })
        .ok_or_else(|| serde::de::Error::custom("number must be a nonnegative safe integer"))?;
    Ok(number as u64)
}

// Original:
//   packages/agent-core-v2/src/agent/contextMemory/protocolMessage.ts
//   messageRoleSchema and messageContentSchema
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ImageSource {
    Url {
        #[serde(deserialize_with = "deserialize_non_empty_string")]
        url: String,
    },
    Base64 {
        #[serde(deserialize_with = "deserialize_non_empty_string")]
        media_type: String,
        #[serde(deserialize_with = "deserialize_non_empty_string")]
        data: String,
    },
    File {
        #[serde(deserialize_with = "deserialize_non_empty_string")]
        file_id: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageContent {
    Text {
        text: String,
    },
    ToolUse {
        #[serde(deserialize_with = "deserialize_non_empty_string")]
        tool_call_id: String,
        #[serde(deserialize_with = "deserialize_non_empty_string")]
        tool_name: String,
        input: Value,
    },
    ToolResult {
        #[serde(deserialize_with = "deserialize_non_empty_string")]
        tool_call_id: String,
        output: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    Image {
        source: ImageSource,
    },
    Video {
        source: ImageSource,
    },
    File {
        #[serde(deserialize_with = "deserialize_non_empty_string")]
        file_id: String,
        name: String,
        #[serde(deserialize_with = "deserialize_non_empty_string")]
        media_type: String,
        #[serde(deserialize_with = "deserialize_nonnegative_safe_integer")]
        size: u64,
    },
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
}

// Original: protocolMessage.ts, messageSchema.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProtocolMessage {
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub id: String,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub session_id: String,
    pub role: MessageRole,
    pub content: Vec<MessageContent>,
    pub created_at: IsoDateTime,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_non_empty_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub prompt_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_non_empty_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn protocol_message_round_trips_external_shape() {
        let value = json!({
            "id": "msg-1",
            "session_id": "session-1",
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "tool_call_id": "call-1",
                    "tool_name": "read_file",
                    "input": { "path": "README.md" }
                },
                {
                    "type": "thinking",
                    "thinking": "",
                    "signature": ""
                }
            ],
            "created_at": "2025-01-02T03:04:05+08:00",
            "metadata": { "source": "test" },
            "unknown": true
        });
        let message: ProtocolMessage = serde_json::from_value(value).unwrap();
        assert_eq!(message.created_at.as_str(), "2025-01-01T19:04:05.000Z");
        assert_eq!(
            serde_json::to_value(message).unwrap(),
            json!({
                "id": "msg-1",
                "session_id": "session-1",
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "tool_call_id": "call-1",
                        "tool_name": "read_file",
                        "input": { "path": "README.md" }
                    },
                    {
                        "type": "thinking",
                        "thinking": "",
                        "signature": ""
                    }
                ],
                "created_at": "2025-01-01T19:04:05.000Z",
                "metadata": { "source": "test" }
            })
        );
    }

    #[test]
    fn rejects_empty_required_strings_and_invalid_dates() {
        let base = json!({
            "id": "msg-1",
            "session_id": "session-1",
            "role": "user",
            "content": [],
            "created_at": "2025-01-01T00:00:00Z"
        });
        for (field, invalid) in [
            ("id", Value::String(String::new())),
            ("session_id", Value::String(String::new())),
            ("created_at", Value::String("not-a-date".into())),
        ] {
            let mut candidate = base.clone();
            candidate[field] = invalid;
            assert!(serde_json::from_value::<ProtocolMessage>(candidate).is_err());
        }
        let mut candidate = base;
        candidate["prompt_id"] = Value::String(String::new());
        assert!(serde_json::from_value::<ProtocolMessage>(candidate).is_err());
    }

    #[test]
    fn file_size_accepts_json_integer_forms_and_rejects_zod_boundaries() {
        fn file(size: Value) -> Value {
            json!({
                "type": "file",
                "file_id": "file-1",
                "name": "x",
                "media_type": "text/plain",
                "size": size
            })
        }

        assert!(serde_json::from_value::<MessageContent>(file(json!(1))).is_ok());
        assert!(serde_json::from_value::<MessageContent>(file(json!(1.0))).is_ok());
        assert!(serde_json::from_value::<MessageContent>(file(json!(-1))).is_err());
        assert!(serde_json::from_value::<MessageContent>(file(json!(1.5))).is_err());
        assert!(
            serde_json::from_value::<MessageContent>(file(json!(MAX_SAFE_INTEGER + 1))).is_err()
        );
    }

    #[test]
    fn direct_construction_is_not_stricter_than_typescript_output_types() {
        let source = ImageSource::Url { url: String::new() };
        assert_eq!(
            serde_json::to_value(source).unwrap(),
            json!({ "kind": "url", "url": "" })
        );
    }
}
