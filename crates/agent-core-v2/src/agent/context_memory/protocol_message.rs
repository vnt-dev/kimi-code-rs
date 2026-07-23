use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::_base::utils::iso_date_time::IsoDateTime;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct NonEmptyString(String);

impl NonEmptyString {
    pub fn new(value: impl Into<String>) -> Result<Self, NonEmptyStringError> {
        let value = value.into();
        if value.is_empty() {
            Err(NonEmptyStringError)
        } else {
            Ok(Self(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl<'de> Deserialize<'de> for NonEmptyString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NonEmptyStringError;

impl fmt::Display for NonEmptyStringError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("string must contain at least 1 character")
    }
}

impl std::error::Error for NonEmptyStringError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct NonNegativeSafeInteger(u64);

impl NonNegativeSafeInteger {
    pub fn new(value: u64) -> Result<Self, NonNegativeSafeIntegerError> {
        if value <= MAX_SAFE_INTEGER {
            Ok(Self(value))
        } else {
            Err(NonNegativeSafeIntegerError)
        }
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for NonNegativeSafeInteger {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
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
            .ok_or_else(|| serde::de::Error::custom(NonNegativeSafeIntegerError))?;
        Self::new(number as u64).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NonNegativeSafeIntegerError;

impl fmt::Display for NonNegativeSafeIntegerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("number must be a nonnegative safe integer")
    }
}

impl std::error::Error for NonNegativeSafeIntegerError {}

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
        url: NonEmptyString,
    },
    Base64 {
        media_type: NonEmptyString,
        data: NonEmptyString,
    },
    File {
        file_id: NonEmptyString,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageContent {
    Text {
        text: String,
    },
    ToolUse {
        tool_call_id: NonEmptyString,
        tool_name: NonEmptyString,
        input: Value,
    },
    ToolResult {
        tool_call_id: NonEmptyString,
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
        file_id: NonEmptyString,
        name: String,
        media_type: NonEmptyString,
        size: NonNegativeSafeInteger,
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
    pub id: NonEmptyString,
    pub session_id: NonEmptyString,
    pub role: MessageRole,
    pub content: Vec<MessageContent>,
    pub created_at: IsoDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_message_id: Option<NonEmptyString>,
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
}
