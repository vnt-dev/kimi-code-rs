//! Plan review and tool-driven plan-mode exit.
//!
//! Original: `agent/plan/tools/exit-plan-mode.ts`.

use std::{
    collections::HashSet,
    sync::{Arc, LazyLock},
};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use kimi_code_protocol::PlanReviewOption;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use crate::{
    _base::di::instantiation::ServicesAccessorExt,
    agent::{
        permission_mode::{AGENT_PERMISSION_MODE_SERVICE_ID, AgentPermissionModeServiceContract},
        permission_policy::PermissionMode,
        plan::{AGENT_PLAN_SERVICE_ID, AgentPlanServiceContract},
        tool_registry::{ToolContributionOptions, register_tool},
    },
    app::telemetry::{
        PlanResolutionOutcome, PlanResolvedEvent, PlanSubmittedEvent, TELEMETRY_SERVICE_ID,
        TelemetryServiceContract, TelemetryServiceEventExt,
    },
    kosong::contract::tool::Tool,
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolExecution, ToolInputDisplay, input_schema::to_input_json_schema,
    },
};

const EXIT_PLAN_MODE_DESCRIPTION: &str = include_str!("exit-plan-mode.md");
const RESERVED_OPTION_LABELS: &[&str] = &["approve", "reject", "reject and exit", "revise"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExitPlanModeOption {
    pub label: String,
    pub description: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExitPlanModeInput {
    pub options: Option<Vec<ExitPlanModeOption>>,
}

impl<'de> Deserialize<'de> for ExitPlanModeInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        parse_exit_plan_mode_input(&Value::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

pub fn parse_exit_plan_mode_input(value: &Value) -> Result<ExitPlanModeInput, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "ExitPlanMode input must be an object".to_owned())?;
    if object.keys().any(|key| key != "options") {
        return Err("ExitPlanMode input contains an unknown property".into());
    }
    let Some(options) = object.get("options") else {
        return Ok(ExitPlanModeInput::default());
    };
    let options = options
        .as_array()
        .ok_or_else(|| "options must be an array".to_owned())?;
    if !(1..=3).contains(&options.len()) {
        return Err("options must contain 1 to 3 options".into());
    }

    let options = options
        .iter()
        .map(|value| {
            let option = value
                .as_object()
                .ok_or_else(|| "each option must be an object".to_owned())?;
            if option
                .keys()
                .any(|key| !matches!(key.as_str(), "label" | "description"))
            {
                return Err("an option contains an unknown property".into());
            }
            let label = option
                .get("label")
                .and_then(Value::as_str)
                .filter(|label| !label.is_empty())
                .ok_or_else(|| "option label must be a non-empty string".to_owned())?;
            if label.chars().count() > 80 {
                return Err("option label must contain at most 80 characters".into());
            }
            let description = match option.get("description") {
                None => String::new(),
                Some(Value::String(description)) => description.clone(),
                Some(_) => return Err("option description must be a string".into()),
            };
            Ok(ExitPlanModeOption {
                label: label.into(),
                description,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let mut labels = HashSet::new();
    for option in &options {
        let label = normalize_option_label(&option.label);
        if !labels.insert(label.clone()) {
            return Err("Option labels must be unique.".into());
        }
        if RESERVED_OPTION_LABELS.contains(&label.as_str()) {
            return Err("Option labels must not use reserved approval labels.".into());
        }
    }
    Ok(ExitPlanModeInput {
        options: Some(options),
    })
}

pub static EXIT_PLAN_MODE_PARAMETERS: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "options": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 3,
                    "description": "When the plan contains multiple alternative approaches, list them here so the user can choose which one to execute. Provide up to 3 options; 2-3 distinct approaches work best when the plan offers a real choice. Passing a single option is allowed and is equivalent to a plain plan approval. Each option represents a distinct approach from the plan. Do not use \"Reject\", \"Revise\", \"Approve\", or \"Reject and Exit\" as labels.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "label": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 80,
                                "description": "Short name for this option (1-8 words). Append \"(Recommended)\" if you recommend this option."
                            },
                            "description": {
                                "type": "string",
                                "default": "",
                                "description": "Brief summary of this approach and its trade-offs."
                            }
                        },
                        "required": ["label"],
                        "additionalProperties": false
                    }
                }
            },
            "additionalProperties": false
        })
        .as_object()
        .cloned()
        .expect("ExitPlanMode schema is an object"),
    )
});

pub struct ExitPlanModeTool {
    plan: Arc<dyn AgentPlanServiceContract>,
    permission: Arc<dyn AgentPermissionModeServiceContract>,
    telemetry: Arc<dyn TelemetryServiceContract>,
    definition: Tool,
}

impl ExitPlanModeTool {
    pub fn new(
        plan: Arc<dyn AgentPlanServiceContract>,
        permission: Arc<dyn AgentPermissionModeServiceContract>,
        telemetry: Arc<dyn TelemetryServiceContract>,
    ) -> Self {
        Self {
            plan,
            permission,
            telemetry,
            definition: Tool {
                name: "ExitPlanMode".into(),
                description: EXIT_PLAN_MODE_DESCRIPTION.into(),
                parameters: EXIT_PLAN_MODE_PARAMETERS.clone(),
                deferred: None,
            },
        }
    }

    async fn resolve_plan_review_display(
        &self,
        args: &ExitPlanModeInput,
    ) -> Option<ToolInputDisplay> {
        let data = self.plan.status().await.ok().flatten()?;
        if data.content.trim().is_empty() {
            return None;
        }
        Some(ToolInputDisplay::PlanReview {
            plan: data.content,
            path: Some(data.path),
            options: args.options.as_ref().and_then(|options| {
                (options.len() >= 2).then(|| {
                    options
                        .iter()
                        .map(|option| PlanReviewOption {
                            label: option.label.clone(),
                            description: option.description.clone(),
                        })
                        .collect()
                })
            }),
        })
    }
}

#[async_trait]
impl ExecutableTool for ExitPlanModeTool {
    type Input = ExitPlanModeInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    async fn resolve_execution(&self, args: ExitPlanModeInput) -> ToolExecution {
        let display = self.resolve_plan_review_display(&args).await;
        let plan = Arc::clone(&self.plan);
        let permission = Arc::clone(&self.permission);
        let telemetry = Arc::clone(&self.telemetry);
        let execute = Arc::new(move |_context: ExecutableToolContext| {
            let args = args.clone();
            let plan = Arc::clone(&plan);
            let permission = Arc::clone(&permission);
            let telemetry = Arc::clone(&telemetry);
            Box::pin(async move {
                execute_exit_plan_mode(
                    plan.as_ref(),
                    permission.as_ref(),
                    telemetry.as_ref(),
                    &args,
                )
                .await
            }) as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new("ExitPlanMode", execute);
        execution.description = Some("Presenting plan and exiting plan mode".into());
        execution.display = display;
        ToolExecution::Runnable(execution)
    }
}

pub fn register_exit_plan_mode_tool() {
    register_tool(
        Arc::new(|accessor| {
            let plan = accessor.get(AGENT_PLAN_SERVICE_ID)?;
            let permission = accessor.get(AGENT_PERMISSION_MODE_SERVICE_ID)?;
            let telemetry = accessor.get(TELEMETRY_SERVICE_ID)?;
            Ok(Arc::new(ExitPlanModeTool::new(
                Arc::clone(&plan.0),
                Arc::clone(&permission.0),
                Arc::clone(&telemetry.0),
            )))
        }),
        ToolContributionOptions::default(),
    );
}

async fn execute_exit_plan_mode(
    plan: &dyn AgentPlanServiceContract,
    permission: &dyn AgentPermissionModeServiceContract,
    telemetry: &dyn TelemetryServiceContract,
    args: &ExitPlanModeInput,
) -> ExecutableToolResult {
    match plan.status().await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return ExecutableToolResult::error(
                "ExitPlanMode can only be called while plan mode is active. Use EnterPlanMode (or /plan) first.",
            );
        }
        Err(error) => {
            return ExecutableToolResult::error(format!("Failed to read plan file: {error}"));
        }
    }

    let (plan_content, plan_path) = match resolve_plan(plan).await {
        Ok(plan) => plan,
        Err(error) => return error,
    };
    let _ = telemetry.track_event(&PlanSubmittedEvent {
        has_options: args
            .options
            .as_ref()
            .is_some_and(|options| options.len() >= 2),
    });

    if let Err(error) = plan.exit(None) {
        return ExecutableToolResult::error(format!("Failed to exit plan mode: {error}"));
    }

    if permission.mode() == PermissionMode::Auto {
        let _ = telemetry.track_event(&PlanResolvedEvent {
            outcome: PlanResolutionOutcome::AutoApproved,
            chosen_option: None,
            has_feedback: None,
        });
        return ExecutableToolResult::success(format!(
            "Exited plan mode. {}",
            format_auto_approved_plan_for_output(&plan_content, plan_path.as_deref())
        ));
    }

    let _ = telemetry.track_event(&PlanResolvedEvent {
        outcome: PlanResolutionOutcome::Approved,
        chosen_option: None,
        has_feedback: None,
    });
    ExecutableToolResult::success(format!(
        "Exited plan mode. {}",
        format_plan_for_output(&plan_content, plan_path.as_deref())
    ))
}

async fn resolve_plan(
    plan: &dyn AgentPlanServiceContract,
) -> Result<(String, Option<String>), ExecutableToolResult> {
    let source = match plan.status().await {
        Ok(data) => data,
        Err(error) => {
            return Err(ExecutableToolResult::error(format!(
                "Failed to read plan file: {error}"
            )));
        }
    };
    if let Some(source) = &source
        && !source.content.trim().is_empty()
    {
        return Ok((source.content.clone(), Some(source.path.clone())));
    }

    let status = plan.status().await.ok().flatten();
    let path = source
        .as_ref()
        .map(|data| data.path.as_str())
        .or_else(|| status.as_ref().map(|data| data.path.as_str()));
    Err(ExecutableToolResult::error(match path {
        None => {
            "No plan file found. Write the plan to the current plan file first, then call ExitPlanMode."
                .into()
        }
        Some(path) => {
            format!("No plan file found. Write your plan to {path} first, then call ExitPlanMode.")
        }
    }))
}

fn normalize_option_label(label: &str) -> String {
    label.trim().to_lowercase()
}

fn format_auto_approved_plan_for_output(plan: &str, path: Option<&str>) -> String {
    let saved_to = path.map_or_else(String::new, |path| format!("Plan saved to: {path}\n\n"));
    format!(
        "Plan mode deactivated. All tools are now available.\nNote: this plan was auto-approved without user review — the user has NOT explicitly approved it. Follow the user's original instructions on whether to proceed with execution; if they asked you to stop, wait, or only summarize after planning, do not start executing.\n{saved_to}## Plan (auto-approved, not user-reviewed):\n{plan}"
    )
}

fn format_plan_for_output(plan: &str, path: Option<&str>) -> String {
    let saved_to = path.map_or_else(String::new, |path| format!("Plan saved to: {path}\n\n"));
    format!(
        "Plan mode deactivated. All tools are now available.\n{saved_to}## Approved Plan:\n{plan}"
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use crate::{
        _base::{
            di::lifecycle::{Disposable, DisposeResult},
            event::Event,
            utils::abort::AbortController,
        },
        agent::{
            permission_mode::{PermissionModeChangedContext, PermissionModeServiceError},
            plan::{PlanData, PlanServiceError},
        },
        app::telemetry::NoopTelemetryService,
    };

    struct StubPlan {
        data: Mutex<Option<PlanData>>,
        exits: AtomicUsize,
    }

    #[async_trait]
    impl AgentPlanServiceContract for StubPlan {
        async fn enter(
            &self,
            _id: Option<String>,
            _create_file: bool,
        ) -> Result<(), PlanServiceError> {
            Ok(())
        }

        fn cancel(&self, _id: Option<String>) -> Result<(), PlanServiceError> {
            Ok(())
        }

        async fn clear(&self) -> Result<(), PlanServiceError> {
            Ok(())
        }

        fn exit(&self, _id: Option<String>) -> Result<(), PlanServiceError> {
            self.exits.fetch_add(1, Ordering::SeqCst);
            *self.data.lock().unwrap() = None;
            Ok(())
        }

        async fn status(&self) -> Result<Option<PlanData>, PlanServiceError> {
            Ok(self.data.lock().unwrap().clone())
        }
    }

    impl Disposable for StubPlan {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    struct StubPermission(PermissionMode);

    impl AgentPermissionModeServiceContract for StubPermission {
        fn mode(&self) -> PermissionMode {
            self.0
        }

        fn set_mode(&self, _mode: PermissionMode) -> Result<(), PermissionModeServiceError> {
            Ok(())
        }

        fn on_did_change_mode(&self) -> Event<PermissionModeChangedContext> {
            Event::none()
        }
    }

    impl Disposable for StubPermission {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    #[test]
    fn input_validation_matches_cardinality_uniqueness_and_reserved_labels() {
        assert!(parse_exit_plan_mode_input(&json!({})).is_ok());
        assert!(
            parse_exit_plan_mode_input(
                &json!({"options":[{"label":"Direct","description":"Simple"}]})
            )
            .is_ok()
        );
        assert!(
            parse_exit_plan_mode_input(&json!({"options":[{"label":"One"},{"label":" one "}]}))
                .unwrap_err()
                .contains("unique")
        );
        assert!(
            parse_exit_plan_mode_input(&json!({"options":[{"label":"Approve"}]}))
                .unwrap_err()
                .contains("reserved")
        );
        assert!(parse_exit_plan_mode_input(&json!({"options":[]})).is_err());
        assert!(
            parse_exit_plan_mode_input(&json!({"options":[{"label":"One","extra":true}]})).is_err()
        );
    }

    #[test]
    fn output_format_distinguishes_reviewed_and_auto_approved_plans() {
        let approved = format_plan_for_output("1. Change it", Some("/plans/p.md"));
        assert!(approved.contains("## Approved Plan:"));
        assert!(approved.contains("Plan saved to: /plans/p.md"));
        let automatic = format_auto_approved_plan_for_output("1. Change it", None);
        assert!(automatic.contains("auto-approved without user review"));
        assert!(automatic.contains("user has NOT explicitly approved it"));
    }

    #[tokio::test]
    async fn review_display_and_auto_execution_read_and_exit_the_active_plan() {
        let plan = Arc::new(StubPlan {
            data: Mutex::new(Some(PlanData {
                id: "plan-1".into(),
                content: "# Plan\n\n1. Change it".into(),
                path: "C:/plans/plan-1.md".into(),
            })),
            exits: AtomicUsize::new(0),
        });
        let tool = ExitPlanModeTool::new(
            plan.clone(),
            Arc::new(StubPermission(PermissionMode::Auto)),
            Arc::new(NoopTelemetryService),
        );
        let args = parse_exit_plan_mode_input(&json!({
            "options": [
                {"label": "Direct", "description": "Small change"},
                {"label": "Layered", "description": "More structure"}
            ]
        }))
        .unwrap();
        let ToolExecution::Runnable(execution) = tool.resolve_execution(args).await else {
            panic!("ExitPlanMode must resolve to a runnable execution");
        };
        assert!(matches!(
            execution.display,
            Some(ToolInputDisplay::PlanReview {
                options: Some(ref options),
                ..
            }) if options.len() == 2
        ));
        let result = execution
            .execute(ExecutableToolContext {
                turn_id: 1,
                tool_call_id: "call-1".into(),
                trace: None,
                metadata: None,
                signal: AbortController::new().signal(),
                on_update: None,
                on_foreground_task_start: None,
            })
            .await;
        assert!(!result.is_error);
        assert!(
            matches!(&result.output, crate::tool::ExecutableToolOutput::Text(output) if output.contains("auto-approved without user review"))
        );
        assert_eq!(plan.exits.load(Ordering::SeqCst), 1);
        assert!(plan.status().await.unwrap().is_none());
    }
}
