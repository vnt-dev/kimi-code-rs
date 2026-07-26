//! `CronList` builtin tool.
//!
//! Original: `packages/agent-core-v2/src/session/cron/tools/cron-list.ts`.

use std::{sync::Arc, sync::LazyLock};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use crate::{
    app::cron::{CronTask, cron_to_human, format_local_iso_with_offset, parse_cron_expression},
    kosong::contract::tool::Tool,
    session::cron::SessionCronServiceHandle,
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolExecution, input_schema::to_input_json_schema,
    },
};

const CRON_LIST_DESCRIPTION: &str = include_str!("cron-list.md");
const MS_PER_DAY: f64 = 24.0 * 60.0 * 60.0 * 1_000.0;
const PROMPT_PREVIEW_BYTES: usize = 200;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CronListInput;

impl<'de> Deserialize<'de> for CronListInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value.as_object() {
            Some(object) if object.is_empty() => Ok(Self),
            Some(_) => Err(serde::de::Error::custom(
                "CronList input contains an unknown property",
            )),
            None => Err(serde::de::Error::custom("CronList input must be an object")),
        }
    }
}

pub static CRON_LIST_PARAMETERS: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    to_input_json_schema(
        serde_json::from_value(json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }))
        .expect("CronList schema is an object"),
    )
});

pub struct CronListTool {
    cron: SessionCronServiceHandle,
    definition: Tool,
}

impl CronListTool {
    pub fn new(cron: SessionCronServiceHandle) -> Self {
        Self {
            cron,
            definition: Tool {
                name: "CronList".into(),
                description: CRON_LIST_DESCRIPTION.into(),
                parameters: CRON_LIST_PARAMETERS.clone(),
                deferred: None,
            },
        }
    }
}

#[async_trait]
impl ExecutableTool for CronListTool {
    type Input = CronListInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    async fn resolve_execution(&self, _args: CronListInput) -> ToolExecution {
        let cron = self.cron.clone();
        let execute = Arc::new(move |_context: ExecutableToolContext| {
            let cron = cron.clone();
            Box::pin(async move {
                let tasks = cron.list();
                let header = format!("cron_jobs: {}", tasks.len());
                if tasks.is_empty() {
                    return ExecutableToolResult::success(format!(
                        "{header}\nNo cron jobs scheduled."
                    ));
                }
                let now_ms = cron.now();
                let records = tasks
                    .iter()
                    .map(|task| render_record(cron.0.as_ref(), task, now_ms))
                    .collect::<Vec<_>>();
                ExecutableToolResult::success(format!("{header}\n{}", records.join("\n---\n")))
            }) as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new("CronList", execute);
        execution.description = Some("Listing scheduled cron jobs".into());
        ToolExecution::Runnable(execution)
    }
}

fn render_record(
    cron: &dyn crate::session::cron::SessionCronServiceContract,
    task: &CronTask,
    now_ms: f64,
) -> String {
    let age_ms = now_ms - task.created_at;
    let age_days = if age_ms.is_finite() {
        age_ms / MS_PER_DAY
    } else {
        0.0
    };
    let mut human_schedule = task.cron.clone();
    let mut next_fire_at = "null".to_owned();
    if let Ok(parsed) = parse_cron_expression(&task.cron) {
        human_schedule = cron_to_human(&parsed);
        if let Some(next) = cron.get_next_fire_for_task(&task.id) {
            next_fire_at = format_local_iso_with_offset(next);
        }
    }
    [
        format!("id: {}", task.id),
        format!("cron: {}", task.cron),
        format!("humanSchedule: {human_schedule}"),
        format!(
            "prompt: {}",
            serde_json::to_string(&preview_prompt(&task.prompt)).expect("string serializes")
        ),
        format!("nextFireAt: {next_fire_at}"),
        format!("recurring: {}", task.recurring != Some(false)),
        format!("ageDays: {age_days:.2}"),
        format!("stale: {}", cron.is_stale(task)),
    ]
    .join("\n")
}

fn preview_prompt(prompt: &str) -> String {
    if prompt.len() <= PROMPT_PREVIEW_BYTES {
        return prompt.into();
    }
    let mut end = PROMPT_PREVIEW_BYTES;
    while !prompt.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…(truncated)", &prompt[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_is_limited_by_utf8_bytes_without_splitting_code_points() {
        let prompt = "你".repeat(100);
        let preview = preview_prompt(&prompt);
        assert!(preview.ends_with("…(truncated)"));
        assert_eq!(preview.trim_end_matches("…(truncated)").len(), 198);
    }

    #[test]
    fn input_is_strictly_empty() {
        assert!(serde_json::from_value::<CronListInput>(json!({})).is_ok());
        assert!(serde_json::from_value::<CronListInput>(json!({"x": 1})).is_err());
    }
}
