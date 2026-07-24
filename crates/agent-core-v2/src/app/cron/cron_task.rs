//! Persistent cron task data shape.
//!
//! Original: `packages/agent-core-v2/src/app/cron/cronTask.ts`.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

// Original: CronTask. Timestamps remain floating-point milliseconds because
// the persistent JavaScript format is a Number rather than an integer-only
// protocol field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronTask {
    pub id: String,
    pub cron: String,
    pub prompt: String,
    pub created_at: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurring: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_fired_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<IndexMap<String, String>>,
}

// Original: CronTaskInit = Omit<CronTask, 'id' | 'createdAt'>.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronTaskInit {
    pub cron: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurring: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_fired_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<IndexMap<String, String>>,
}

// Original: CRON_SESSION_TAG.
pub const CRON_SESSION_TAG: &str = "sessionId";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_preserves_the_persistent_camel_case_shape_and_optional_fields() {
        let task: CronTask = serde_json::from_value(serde_json::json!({
            "id": "task-1",
            "cron": "0 9 * * *",
            "prompt": "Summarize today",
            "createdAt": 1730000000000.5,
            "recurring": true,
            "lastFiredAt": 1730000100000.25,
            "tags": {"sessionId": "session-1"}
        }))
        .unwrap();
        assert_eq!(task.created_at, 1_730_000_000_000.5);
        assert_eq!(task.tags.as_ref().unwrap()[CRON_SESSION_TAG], "session-1");
        assert_eq!(
            serde_json::to_value(task).unwrap()["lastFiredAt"],
            1_730_000_100_000.25
        );
    }

    #[test]
    fn init_excludes_generated_fields_and_omits_missing_optionals() {
        let init = CronTaskInit {
            cron: "*/5 * * * *".into(),
            prompt: "Check status".into(),
            recurring: None,
            last_fired_at: None,
            tags: None,
        };
        assert_eq!(
            serde_json::to_value(init).unwrap(),
            serde_json::json!({"cron": "*/5 * * * *", "prompt": "Check status"})
        );
    }
}
