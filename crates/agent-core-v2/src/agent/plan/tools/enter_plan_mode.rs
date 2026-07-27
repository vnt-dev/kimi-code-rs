//! Tool-driven plan-mode entry.
//!
//! Original: `agent/plan/tools/enter-plan-mode.ts`.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    _base::di::instantiation::ServicesAccessorExt,
    agent::{
        plan::{AGENT_PLAN_SERVICE_ID, AgentPlanServiceContract},
        tool_registry::{ToolContributionOptions, register_tool},
    },
    app::telemetry::{
        PlanEnterOutcome, PlanEnterResolvedEvent, TELEMETRY_SERVICE_ID, TelemetryServiceContract,
        TelemetryServiceEventExt,
    },
    kosong::contract::tool::Tool,
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolExecution, input_schema::to_input_json_schema,
    },
};

const ENTER_PLAN_MODE_DESCRIPTION: &str = include_str!("enter-plan-mode.md");

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EnterPlanModeInput {}

pub static ENTER_PLAN_MODE_PARAMETERS: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
        .as_object()
        .cloned()
        .expect("EnterPlanMode schema is an object"),
    )
});

pub struct EnterPlanModeTool {
    plan: Arc<dyn AgentPlanServiceContract>,
    telemetry: Arc<dyn TelemetryServiceContract>,
    definition: Tool,
}

impl EnterPlanModeTool {
    pub fn new(
        plan: Arc<dyn AgentPlanServiceContract>,
        telemetry: Arc<dyn TelemetryServiceContract>,
    ) -> Self {
        Self {
            plan,
            telemetry,
            definition: Tool {
                name: "EnterPlanMode".into(),
                description: ENTER_PLAN_MODE_DESCRIPTION.into(),
                parameters: ENTER_PLAN_MODE_PARAMETERS.clone(),
                deferred: None,
            },
        }
    }
}

#[async_trait]
impl ExecutableTool for EnterPlanModeTool {
    type Input = EnterPlanModeInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    async fn resolve_execution(&self, _args: EnterPlanModeInput) -> ToolExecution {
        let plan = Arc::clone(&self.plan);
        let telemetry = Arc::clone(&self.telemetry);
        let execute = Arc::new(move |_context: ExecutableToolContext| {
            let plan = Arc::clone(&plan);
            let telemetry = Arc::clone(&telemetry);
            Box::pin(async move {
                match plan.status().await {
                    Ok(Some(_)) => {
                        return ExecutableToolResult::error(
                            "Plan mode is already active. Use ExitPlanMode when the plan is ready.",
                        );
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return ExecutableToolResult::error(format!(
                            "Failed to enter plan mode: {error}"
                        ));
                    }
                }

                if let Err(error) = plan.enter(None, false).await {
                    return ExecutableToolResult::error(format!(
                        "Failed to enter plan mode: {error}"
                    ));
                }

                let _ = telemetry.track_event(&PlanEnterResolvedEvent {
                    outcome: PlanEnterOutcome::AutoApproved,
                });
                let path = plan.status().await.ok().flatten().map(|data| data.path);
                ExecutableToolResult::success(entered_plan_mode_message(path.as_deref()))
            }) as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new("EnterPlanMode", execute);
        execution.description = Some("Requesting to enter plan mode".into());
        ToolExecution::Runnable(execution)
    }
}

pub fn register_enter_plan_mode_tool() {
    register_tool(
        Arc::new(|accessor| {
            let plan = accessor.get(AGENT_PLAN_SERVICE_ID)?;
            let telemetry = accessor.get(TELEMETRY_SERVICE_ID)?;
            Ok(Arc::new(EnterPlanModeTool::new(
                Arc::clone(&plan.0),
                Arc::clone(&telemetry.0),
            )))
        }),
        ToolContributionOptions::default(),
    );
}

fn entered_plan_mode_message(plan_path: Option<&str>) -> String {
    let lines = if let Some(plan_path) = plan_path {
        vec![
            "Plan mode is now active. Your workflow:".into(),
            String::new(),
            format!("Plan file: {plan_path}"),
            String::new(),
            "1. Use read-only tools (Read, Grep, Glob) to investigate the codebase. Use Bash only when needed.".into(),
            "2. Design a concrete, step-by-step plan.".into(),
            "3. Write the plan to the plan file with Write or Edit.".into(),
            "4. When the plan is ready, call ExitPlanMode for user approval.".into(),
            String::new(),
            "Do NOT edit files other than the plan file while plan mode is active.".into(),
            "Use Bash only when needed; Bash follows the normal permission mode and rules.".into(),
        ]
    } else {
        vec![
            "Plan mode is now active. Your workflow:".into(),
            String::new(),
            "1. Use read-only tools (Read, Grep, Glob) to investigate the codebase. Use Bash only when needed.".into(),
            "2. Design a concrete, step-by-step plan.".into(),
            "3. Wait for the host to provide a plan file path before calling ExitPlanMode.".into(),
            String::new(),
            "Do NOT use Write or Edit while plan mode is active in this host; no plan file path is available.".into(),
            "Use Bash only when needed; Bash follows the normal permission mode and rules.".into(),
        ]
    };
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_empty_input_and_entry_messages_match_the_source() {
        assert!(serde_json::from_value::<EnterPlanModeInput>(json!({})).is_ok());
        assert!(serde_json::from_value::<EnterPlanModeInput>(json!({"extra": true})).is_err());
        assert_eq!(
            ENTER_PLAN_MODE_PARAMETERS["additionalProperties"],
            Value::Bool(false)
        );
        assert!(
            entered_plan_mode_message(Some("C:/plans/one.md"))
                .contains("Plan file: C:/plans/one.md")
        );
        assert!(
            entered_plan_mode_message(None)
                .contains("Wait for the host to provide a plan file path")
        );
    }
}
