//! Plan-review approval, resolution, and telemetry before leaving plan mode.
//!
//! Original: `agent/permissionPolicy/policies/exit-plan-mode-review-ask.ts`.

use std::sync::Arc;

use futures_util::FutureExt;

use crate::{
    agent::{
        permission_mode::AgentPermissionModeServiceContract,
        permission_policy::{
            PermissionMode, PermissionPolicy, PermissionPolicyFuture, PermissionPolicyResolution,
            PermissionPolicyResult,
        },
        plan::AgentPlanServiceContract,
        tool_executor::{PrepareToolExecutionResult, ResolvedToolExecutionHookContext},
    },
    app::telemetry::{
        PlanResolutionOutcome, PlanResolvedEvent, PlanSubmittedEvent, TelemetryServiceContract,
        TelemetryServiceEventExt,
    },
    session::approval::{ApprovalDecision, ApprovalResponse},
    tool::{ExecutableToolResult, ToolInputDisplay},
};

pub struct ExitPlanModeReviewAskPermissionPolicy {
    plan: Arc<dyn AgentPlanServiceContract>,
    mode: Arc<dyn AgentPermissionModeServiceContract>,
    telemetry: Arc<dyn TelemetryServiceContract>,
}
impl ExitPlanModeReviewAskPermissionPolicy {
    pub fn new(
        plan: Arc<dyn AgentPlanServiceContract>,
        mode: Arc<dyn AgentPermissionModeServiceContract>,
        telemetry: Arc<dyn TelemetryServiceContract>,
    ) -> Self {
        Self {
            plan,
            mode,
            telemetry,
        }
    }
}

#[derive(Clone)]
struct PlanReview {
    plan: String,
    path: Option<String>,
    options: Option<Vec<kimi_code_protocol::PlanReviewOption>>,
}

fn prepared(result: ExecutableToolResult) -> PermissionPolicyResolution {
    PermissionPolicyResolution::Prepared(Box::new(PrepareToolExecutionResult {
        block: None,
        reason: None,
        synthetic_result: Some(result),
        execution_metadata: None,
        updated_args: None,
    }))
}
fn track(telemetry: &dyn TelemetryServiceContract, event: PlanResolvedEvent) {
    let _ = telemetry.track_event(&event);
}

fn resolve(
    plan_service: &dyn AgentPlanServiceContract,
    telemetry: &dyn TelemetryServiceContract,
    result: ApprovalResponse,
    display: PlanReview,
) -> PermissionPolicyResolution {
    if result.decision == ApprovalDecision::Approved {
        let selected = display.options.as_ref().and_then(|options| {
            result
                .selected_label
                .as_ref()
                .and_then(|label| options.iter().find(|option| option.label == *label))
        });
        let _ = plan_service.exit(None);
        track(
            telemetry,
            PlanResolvedEvent {
                outcome: PlanResolutionOutcome::Approved,
                chosen_option: result
                    .selected_label
                    .clone()
                    .filter(|label| !label.is_empty()),
                has_feedback: None,
            },
        );
        let prefix = selected.map_or_else(String::new, |option| format!("Selected approach: {}\nExecute ONLY the selected approach. Do not execute any unselected alternatives.\n\n", option.label));
        let saved = display
            .path
            .map_or_else(String::new, |path| format!("Plan saved to: {path}\n\n"));
        return prepared(ExecutableToolResult::success(format!(
            "Exited plan mode. {prefix}Plan mode deactivated. All tools are now available.\n{saved}## Approved Plan:\n{}",
            display.plan
        )));
    }
    if result.decision == ApprovalDecision::Cancelled {
        track(
            telemetry,
            PlanResolvedEvent {
                outcome: PlanResolutionOutcome::Dismissed,
                chosen_option: None,
                has_feedback: None,
            },
        );
        return prepared(ExecutableToolResult::success(
            "Plan approval dismissed. Plan mode remains active.",
        ));
    }
    if result.selected_label.as_deref() == Some("Reject and Exit") {
        let _ = plan_service.exit(None);
        track(
            telemetry,
            PlanResolvedEvent {
                outcome: PlanResolutionOutcome::RejectedAndExited,
                chosen_option: None,
                has_feedback: None,
            },
        );
        let mut output =
            ExecutableToolResult::error("Plan rejected by user. Plan mode deactivated.");
        output.stop_turn = Some(true);
        return prepared(output);
    }
    let feedback = result.feedback.unwrap_or_default();
    if result.selected_label.as_deref() == Some("Revise") || !feedback.is_empty() {
        track(
            telemetry,
            PlanResolvedEvent {
                outcome: PlanResolutionOutcome::Revise,
                chosen_option: None,
                has_feedback: Some(!feedback.is_empty()),
            },
        );
        return prepared(ExecutableToolResult::success(if feedback.is_empty() {
            "User requested revisions. Plan mode remains active.".into()
        } else {
            format!("User rejected the plan. Feedback:\n\n{feedback}")
        }));
    }
    track(
        telemetry,
        PlanResolvedEvent {
            outcome: PlanResolutionOutcome::Rejected,
            chosen_option: None,
            has_feedback: None,
        },
    );
    let mut output =
        ExecutableToolResult::error("Plan rejected by user. Plan mode remains active.");
    output.stop_turn = Some(true);
    prepared(output)
}

impl PermissionPolicy for ExitPlanModeReviewAskPermissionPolicy {
    fn name(&self) -> &str {
        "exit-plan-mode-review-ask"
    }
    fn evaluate<'a>(
        &'a self,
        context: &'a ResolvedToolExecutionHookContext,
    ) -> PermissionPolicyFuture<'a> {
        async move {
            if context.tool_call.name != "ExitPlanMode"
                || self.mode.mode() == PermissionMode::Auto
                || self.plan.status().await.ok()?.is_none()
            {
                return None;
            }
            let Some(ToolInputDisplay::PlanReview {
                plan,
                path,
                options,
            }) = &context.execution.display
            else {
                return None;
            };
            if plan.trim().is_empty() {
                return None;
            }
            let _ = self.telemetry.track_event(&PlanSubmittedEvent {
                has_options: options.as_ref().is_some_and(|options| options.len() >= 2),
            });
            let display = PlanReview {
                plan: plan.clone(),
                path: path.clone(),
                options: options.clone(),
            };
            let service = Arc::clone(&self.plan);
            let telemetry = Arc::clone(&self.telemetry);
            Some(PermissionPolicyResult::Ask {
                reason: Some(std::collections::BTreeMap::from([(
                    "has_options".into(),
                    crate::agent::permission_policy::PermissionReasonValue::Boolean(
                        display.options.is_some(),
                    ),
                )])),
                resolve_error: None,
                resolve_approval: Some(Box::new(move |result| {
                    Some(resolve(
                        service.as_ref(),
                        telemetry.as_ref(),
                        result,
                        display.clone(),
                    ))
                })),
            })
        }
        .boxed()
    }
}
