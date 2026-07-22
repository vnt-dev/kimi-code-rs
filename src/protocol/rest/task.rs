use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::protocol::validation::{literal_false, literal_true};
use crate::protocol::{Task, TaskStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ListTasksQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListTasksResponse {
    pub items: Vec<Task>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GetTaskQuery {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_coerced_bool"
    )]
    pub with_output: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_coerced_u64"
    )]
    pub output_bytes: Option<u64>,
}

fn deserialize_coerced_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(Some(match value {
        Value::Null => false,
        Value::Bool(value) => value,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }))
}

fn deserialize_coerced_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    let number = match value {
        Value::Null => 0.0,
        Value::Bool(value) => f64::from(u8::from(value)),
        Value::Number(value) => value.as_f64().unwrap_or(f64::NAN),
        Value::String(value) if value.trim().is_empty() => 0.0,
        Value::String(value) => value.parse().map_err(serde::de::Error::custom)?,
        Value::Array(_) | Value::Object(_) => {
            return Err(serde::de::Error::custom("cannot coerce value to number"));
        }
    };
    if number.is_finite() && number >= 0.0 && number.fract() == 0.0 && number <= u64::MAX as f64 {
        Ok(Some(number as u64))
    } else {
        Err(serde::de::Error::custom(
            "output_bytes must be a nonnegative integer",
        ))
    }
}

pub type GetTaskResponse = Task;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelTaskResult {
    #[serde(deserialize_with = "literal_true")]
    pub cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskAlreadyFinishedData {
    #[serde(deserialize_with = "literal_false")]
    pub cancelled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::rest::{ApprovalResolveResult, RefreshProviderModelsResponse};

    #[test]
    fn domain_rest_adapters_preserve_aliases_literals_and_query_coercion() {
        let query: GetTaskQuery = serde_json::from_value(serde_json::json!({
            "with_output":"false","output_bytes":"1024"
        }))
        .unwrap();
        // z.coerce.boolean uses JavaScript truthiness: non-empty "false" is true.
        assert_eq!(query.with_output, Some(true));
        assert_eq!(query.output_bytes, Some(1024));
        assert!(
            serde_json::from_value::<ApprovalResolveResult>(serde_json::json!({
                "resolved":false,"resolved_at":"2026-06-04T10:30:00Z"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<RefreshProviderModelsResponse>(serde_json::json!({
                "changed":[],"unchanged":[""],"failed":[]
            }))
            .is_err()
        );
    }
}
