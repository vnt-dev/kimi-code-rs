//! `CronDelete` builtin tool.
//!
//! Original: `packages/agent-core-v2/src/session/cron/tools/cron-delete.ts`.

use std::{sync::Arc, sync::LazyLock};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use crate::{
    app::cron::CRON_ID_REGEX,
    kosong::contract::tool::Tool,
    session::cron::SessionCronServiceHandle,
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolExecution, input_schema::to_input_json_schema,
    },
};

const CRON_DELETE_DESCRIPTION: &str = include_str!("cron-delete.md");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CronDeleteInput {
    pub id: String,
}

impl<'de> Deserialize<'de> for CronDeleteInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("CronDelete input must be an object"))?;
        if object.keys().any(|key| key != "id") {
            return Err(serde::de::Error::custom(
                "CronDelete input contains an unknown property",
            ));
        }
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| serde::de::Error::custom("id must be a string"))?;
        Ok(Self { id: id.into() })
    }
}

pub static CRON_DELETE_PARAMETERS: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    to_input_json_schema(
        serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The cron job id (ULID) returned by CronCreate / CronList."
                }
            },
            "required": ["id"],
            "additionalProperties": false
        }))
        .expect("CronDelete schema is an object"),
    )
});

pub struct CronDeleteTool {
    cron: SessionCronServiceHandle,
    agent_id: String,
    definition: Tool,
}

impl CronDeleteTool {
    pub fn new(cron: SessionCronServiceHandle, agent_id: impl Into<String>) -> Self {
        Self {
            cron,
            agent_id: agent_id.into(),
            definition: Tool {
                name: "CronDelete".into(),
                description: CRON_DELETE_DESCRIPTION.into(),
                parameters: CRON_DELETE_PARAMETERS.clone(),
                deferred: None,
            },
        }
    }
}

#[async_trait]
impl ExecutableTool for CronDeleteTool {
    type Input = CronDeleteInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    async fn resolve_execution(&self, args: CronDeleteInput) -> ToolExecution {
        if !CRON_ID_REGEX.is_match(&args.id) {
            return ToolExecution::Error(ExecutableToolResult::error(format!(
                "Invalid cron job id {} — must be a ULID.",
                serde_json::to_string(&args.id).expect("string serializes")
            )));
        }

        let id = args.id;
        let cron = self.cron.clone();
        let agent_id = self.agent_id.clone();
        let execute_id = id.clone();
        let execute = Arc::new(move |_context: ExecutableToolContext| {
            let cron = cron.clone();
            let agent_id = agent_id.clone();
            let id = execute_id.clone();
            Box::pin(async move {
                let removed = match cron.remove_tasks(std::slice::from_ref(&id)) {
                    Ok(removed) => removed,
                    Err(error) => return ExecutableToolResult::error(error.to_string()),
                };
                if removed.is_empty() {
                    return ExecutableToolResult::error(format!("No cron job with id {id}."));
                }
                cron.emit_deleted(&id, Some(&agent_id));
                ExecutableToolResult::success(format!("Deleted cron job {id}."))
            }) as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new("CronDelete", execute);
        execution.description = Some(format!("Deleting cron {id}"));
        ToolExecution::Runnable(execution)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_requires_a_string_id_and_rejects_extra_properties() {
        assert_eq!(
            serde_json::from_value::<CronDeleteInput>(json!({"id": "deadbeef"})).unwrap(),
            CronDeleteInput {
                id: "deadbeef".into()
            }
        );
        assert!(serde_json::from_value::<CronDeleteInput>(json!({})).is_err());
        assert!(
            serde_json::from_value::<CronDeleteInput>(json!({"id": "deadbeef", "extra": true}))
                .is_err()
        );
    }

    #[test]
    fn schema_and_description_preserve_the_source_contract() {
        assert_eq!(CRON_DELETE_PARAMETERS["additionalProperties"], false);
        assert!(CRON_DELETE_DESCRIPTION.starts_with("Cancel a scheduled cron job by id."));
    }
}
