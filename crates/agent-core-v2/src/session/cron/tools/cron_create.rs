//! `CronCreate` builtin tool.
//!
//! Original: `packages/agent-core-v2/src/session/cron/tools/cron-create.ts`.

use std::{sync::Arc, sync::LazyLock};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use crate::{
    app::cron::{
        CronTaskInit, compute_next_cron_run, cron_to_human, format_local_iso_with_offset,
        has_fire_within_years, parse_cron_expression,
    },
    kosong::contract::tool::Tool,
    session::cron::SessionCronServiceHandle,
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolExecution, input_schema::to_input_json_schema, rule_match::literal_rule_pattern,
    },
};

const CRON_CREATE_DESCRIPTION: &str = include_str!("cron-create.md");
pub const MAX_CRON_JOBS_PER_SESSION: usize = 50;
pub const MAX_CRON_PROMPT_BYTES: usize = 8 * 1024;
pub const ONE_SHOT_MAX_FUTURE_MS: f64 = 350.0 * 24.0 * 60.0 * 60.0 * 1_000.0;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CronCreateInput {
    pub cron: String,
    #[serde(deserialize_with = "deserialize_prompt")]
    pub prompt: String,
    #[serde(default = "default_true", deserialize_with = "deserialize_recurring")]
    pub recurring: bool,
}

fn default_true() -> bool {
    true
}

fn deserialize_prompt<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let prompt = String::deserialize(deserializer)
        .map_err(|_| serde::de::Error::custom("prompt must be a non-empty string"))?;
    if prompt.is_empty() {
        return Err(serde::de::Error::custom(
            "prompt must be a non-empty string",
        ));
    }
    Ok(prompt)
}

fn deserialize_recurring<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::Bool(value) => Ok(value),
        _ => Err(serde::de::Error::custom("recurring must be a boolean")),
    }
}

pub fn parse_cron_create_input(value: &Value) -> Result<CronCreateInput, String> {
    serde_json::from_value(value.clone()).map_err(|error| error.to_string())
}

pub static CRON_CREATE_PARAMETERS: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    to_input_json_schema(serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "cron": {
                "type": "string",
                "description": "5-field cron expression in local time: \"M H DoM Mon DoW\" (e.g. \"*/5 * * * *\" = every 5 minutes; \"30 14 28 2 *\" = Feb 28 at 2:30pm local — a pinned date like this repeats yearly unless you also pass recurring: false)."
            },
            "prompt": {
                "type": "string",
                "minLength": 1,
                "maxLength": 8192,
                "description": "The prompt to enqueue at each fire time. Limited to 8 KiB (UTF-8)."
            },
            "recurring": {
                "type": "boolean",
                "default": true,
                "description": "true (default) = fire on every cron match until deleted or auto-expired after 7 days. false = fire once at the next match, then auto-delete. Use false for \"remind me at X\" one-shot requests with pinned minute/hour/dom/month."
            }
        },
        "required": ["cron", "prompt"],
        "additionalProperties": false
    })).expect("CronCreate schema is an object"))
});

pub struct CronCreateTool {
    cron: SessionCronServiceHandle,
    agent_id: String,
    definition: Tool,
}

impl CronCreateTool {
    pub fn new(cron: SessionCronServiceHandle, agent_id: impl Into<String>) -> Self {
        Self {
            cron,
            agent_id: agent_id.into(),
            definition: Tool {
                name: "CronCreate".into(),
                description: CRON_CREATE_DESCRIPTION.into(),
                parameters: CRON_CREATE_PARAMETERS.clone(),
                deferred: None,
            },
        }
    }
}

#[async_trait]
impl ExecutableTool for CronCreateTool {
    type Input = CronCreateInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    async fn resolve_execution(&self, args: CronCreateInput) -> ToolExecution {
        if self.cron.is_disabled() {
            return ToolExecution::Error(ExecutableToolResult::error(
                "Cron scheduling is disabled (KIMI_DISABLE_CRON=1).",
            ));
        }

        let normalized_cron = args.cron.split_whitespace().collect::<Vec<_>>().join(" ");
        let parsed = match parse_cron_expression(&normalized_cron) {
            Ok(parsed) => parsed,
            Err(error) => {
                return ToolExecution::Error(ExecutableToolResult::error(format!(
                    "Invalid cron expression: {error}"
                )));
            }
        };
        let now_at_prepare = self.cron.now();
        if !has_fire_within_years(&parsed, 5.0, now_at_prepare) {
            return ToolExecution::Error(ExecutableToolResult::error(format!(
                "Cron expression {} has no fire within 5 years; refusing to schedule.",
                serde_json::to_string(&normalized_cron).expect("string serializes")
            )));
        }
        if self.cron.list().len() >= MAX_CRON_JOBS_PER_SESSION {
            return ToolExecution::Error(ExecutableToolResult::error(cap_error()));
        }
        let byte_len = args.prompt.len();
        if byte_len > MAX_CRON_PROMPT_BYTES {
            return ToolExecution::Error(ExecutableToolResult::error(format!(
                "Prompt exceeds {MAX_CRON_PROMPT_BYTES} bytes (got {byte_len})."
            )));
        }
        if !args.recurring
            && let Some(first_fire) = compute_next_cron_run(&parsed, now_at_prepare)
            && first_fire - now_at_prepare > ONE_SHOT_MAX_FUTURE_MS
        {
            return ToolExecution::Error(ExecutableToolResult::error(format!(
                "One-shot cron {} would not fire until {} (more than a year out). If you meant \"today\" or a near date, the pinned day/month has already passed this year — pick a future date or use wildcards.",
                serde_json::to_string(&normalized_cron).expect("string serializes"),
                format_local_iso_with_offset(first_fire)
            )));
        }

        let approval_subject = serde_json::to_string(&json!({
            "cron": normalized_cron,
            "prompt": args.prompt,
            "recurring": args.recurring,
        }))
        .expect("approval input serializes");
        let cron = self.cron.clone();
        let agent_id = self.agent_id.clone();
        let execution_cron = normalized_cron.clone();
        let execution_prompt = args.prompt.clone();
        let execution_parsed = parsed.clone();
        let recurring = args.recurring;
        let execute = Arc::new(move |_context: ExecutableToolContext| {
            let cron = cron.clone();
            let agent_id = agent_id.clone();
            let normalized_cron = execution_cron.clone();
            let prompt = execution_prompt.clone();
            let parsed = execution_parsed.clone();
            Box::pin(async move {
                if cron.list().len() >= MAX_CRON_JOBS_PER_SESSION {
                    return ExecutableToolResult::error(cap_error());
                }
                let now_ms = cron.now();
                let task = match cron.add_task(CronTaskInit {
                    cron: normalized_cron.clone(),
                    prompt,
                    recurring: Some(recurring),
                    last_fired_at: None,
                    tags: None,
                }) {
                    Ok(task) => task,
                    Err(error) => return ExecutableToolResult::error(error.to_string()),
                };
                let next_fire_at = compute_next_cron_run(&parsed, now_ms)
                    .and_then(|ideal| cron.compute_display_next_fire(&task, &parsed, ideal));
                cron.emit_scheduled(&task, Some(&agent_id));
                ExecutableToolResult::success(format_create_output(
                    &task.id,
                    &normalized_cron,
                    &cron_to_human(&parsed),
                    recurring,
                    next_fire_at,
                ))
            }) as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new(
            literal_rule_pattern("CronCreate", &approval_subject),
            execute,
        );
        execution.description = Some(if recurring {
            format!("Scheduling cron {normalized_cron}")
        } else {
            format!("Scheduling one-shot {normalized_cron}")
        });
        ToolExecution::Runnable(execution)
    }
}

fn cap_error() -> String {
    format!("Cron job cap reached (max {MAX_CRON_JOBS_PER_SESSION} per session).")
}

fn format_create_output(
    id: &str,
    cron: &str,
    human_schedule: &str,
    recurring: bool,
    next_fire_at: Option<f64>,
) -> String {
    format!(
        "id: {id}\ncron: {cron}\nhumanSchedule: {human_schedule}\nrecurring: {recurring}\nnextFireAt: {}",
        next_fire_at.map_or_else(
            || "null".into(),
            crate::app::cron::format_local_iso_with_offset
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_defaults_recurring_and_rejects_unknown_fields() {
        assert!(
            parse_cron_create_input(&json!({"cron":"* * * * *","prompt":"x"}))
                .unwrap()
                .recurring
        );
        assert!(
            parse_cron_create_input(&json!({"cron":"* * * * *","prompt":"x","unknown":true}))
                .is_err()
        );
        assert!(parse_cron_create_input(&json!({"cron":"* * * * *","prompt":""})).is_err());
    }

    #[test]
    fn output_shape_matches_source() {
        assert_eq!(
            format_create_output("deadbeef", "*/5 * * * *", "every 5 minutes", true, None),
            "id: deadbeef\ncron: */5 * * * *\nhumanSchedule: every 5 minutes\nrecurring: true\nnextFireAt: null"
        );
    }
}
