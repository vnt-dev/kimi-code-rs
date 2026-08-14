//! Policy-driven authorization before executable tools are run.
//!
//! Original:
//! `packages/agent-core-v2/src/agent/permissionGate/permissionGateService.ts`.

use std::{ops::Deref, sync::Arc, time::Instant};

use futures_util::future::BoxFuture;
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::{ServiceIdentifier, ServicesAccessorExt},
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        lifecycle::lifecycle_machine::BoxError,
        utils::abort::{AbortError, abortable},
    },
    agent::{
        permission_mode::{
            AGENT_PERMISSION_MODE_SERVICE_ID, AgentPermissionModeServiceContract,
            AgentPermissionModeServiceHandle,
        },
        permission_policy::{
            AGENT_PERMISSION_POLICY_SERVICE_ID, AgentPermissionPolicyServiceHandle,
            PermissionDecisionReason, PermissionMode, PermissionPolicyResolution,
            PermissionPolicyResult,
        },
        permission_rules::{
            AGENT_PERMISSION_RULES_SERVICE_ID, AgentPermissionRulesServiceContract,
            AgentPermissionRulesServiceHandle, PermissionApprovalResultRecord, PermissionRule,
        },
        scope_context::{AGENT_SCOPE_CONTEXT_ID, AgentScopeContext},
        tool_executor::{
            AGENT_TOOL_EXECUTOR_SERVICE_ID, AgentToolExecutorServiceHandle,
            AuthorizeToolExecutionResult, ResolvedToolExecutionHookContext,
            ToolBeforeExecuteContext,
        },
    },
    app::{
        event::event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID, EventBusHandle},
        telemetry::{
            PermissionApprovalResult as TelemetryApprovalResult, PermissionApprovalResultEvent,
            PermissionDecision as TelemetryPermissionDecision, PermissionPolicyDecisionEvent,
            TELEMETRY_SERVICE_ID, TelemetryPermissionMode, TelemetryServiceEventExt,
            TelemetryServiceHandle,
        },
    },
    hooks::HookRegisterOptions,
    session::{
        approval::{
            ApprovalDecision, ApprovalRequest, ApprovalResponse, ApprovalScope,
            SESSION_APPROVAL_SERVICE_ID, SessionApprovalServiceHandle,
        },
        session_context::{SESSION_CONTEXT_ID, SessionContext},
    },
    tool::ToolInputDisplay,
};

#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
pub struct PermissionData {
    pub mode: PermissionMode,
    pub rules: Vec<PermissionRule>,
}

#[derive(Debug, thiserror::Error)]
pub enum PermissionGateError {
    #[error(transparent)]
    Approval(#[from] serde_json::Error),
    #[error(transparent)]
    Rules(#[from] crate::agent::permission_rules::PermissionRulesServiceError),
    #[error("{0}")]
    Aborted(Arc<AbortError>),
}

pub trait AgentPermissionGateContract: Disposable + Send + Sync {
    fn data(&self) -> PermissionData;
    fn authorize<'a>(
        &'a self,
        context: &'a ResolvedToolExecutionHookContext,
    ) -> BoxFuture<'a, Result<Option<AuthorizeToolExecutionResult>, PermissionGateError>>;
}

#[derive(Clone)]
pub struct AgentPermissionGateHandle(pub Arc<dyn AgentPermissionGateContract>);

impl Deref for AgentPermissionGateHandle {
    type Target = dyn AgentPermissionGateContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentPermissionGateHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const AGENT_PERMISSION_GATE_ID: ServiceIdentifier<AgentPermissionGateHandle> =
    ServiceIdentifier::new("agentPermissionGate");

pub struct AgentPermissionGate {
    scope: AgentScopeContext,
    mode: Arc<dyn AgentPermissionModeServiceContract>,
    rules: Arc<dyn AgentPermissionRulesServiceContract>,
    policy: AgentPermissionPolicyServiceHandle,
    session: SessionContext,
    approval: Option<SessionApprovalServiceHandle>,
    telemetry: TelemetryServiceHandle,
    event_bus: EventBusHandle,
    disposables: DisposableStore,
}

impl AgentPermissionGate {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scope: AgentScopeContext,
        mode: Arc<dyn AgentPermissionModeServiceContract>,
        rules: Arc<dyn AgentPermissionRulesServiceContract>,
        policy: AgentPermissionPolicyServiceHandle,
        session: SessionContext,
        approval: Option<SessionApprovalServiceHandle>,
        telemetry: TelemetryServiceHandle,
        event_bus: EventBusHandle,
        tool_executor: AgentToolExecutorServiceHandle,
    ) -> Result<Arc<Self>, crate::hooks::HookRegistrationError> {
        let service = Arc::new(Self {
            scope,
            mode,
            rules,
            policy,
            session,
            approval,
            telemetry,
            event_bus,
            disposables: DisposableStore::new(),
        });
        let weak = Arc::downgrade(&service);
        let registration = tool_executor.hooks().on_before_execute_tool.register(
            "permission",
            Arc::new(move |context: &mut ToolBeforeExecuteContext, next| {
                let weak = weak.clone();
                Box::pin(async move {
                    let Some(service) = weak.upgrade() else {
                        return next(context).await;
                    };
                    let result = service
                        .authorize(&context.resolved)
                        .await
                        .map_err(|error| match error {
                            PermissionGateError::Aborted(reason) => {
                                Box::new((*reason).clone()) as BoxError
                            }
                            error => Box::new(error) as BoxError,
                        })?;
                    if let Some(result) = result {
                        let stop = result.block == Some(true) || result.synthetic_result.is_some();
                        context.decision = Some(result);
                        if stop {
                            return Ok(());
                        }
                    }
                    next(context).await
                })
            }),
            HookRegisterOptions::default(),
        )?;
        service.disposables.add(registration);
        Ok(service)
    }

    fn resolve_policy_result<'a>(
        &'a self,
        result: &'a PermissionPolicyResult,
        context: &'a ResolvedToolExecutionHookContext,
        policy_name: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<AuthorizeToolExecutionResult>, PermissionGateError>> {
        Box::pin(async move {
            match result {
                PermissionPolicyResult::Approve {
                    execution_metadata, ..
                } => Ok(execution_metadata
                    .as_ref()
                    .map(|metadata| AuthorizeToolExecutionResult {
                        execution_metadata: Some(metadata.clone()),
                        ..AuthorizeToolExecutionResult::default()
                    })),
                PermissionPolicyResult::Deny { message, .. } => {
                    Ok(Some(AuthorizeToolExecutionResult {
                        block: Some(true),
                        reason: Some(self.format_deny_message(message.clone().unwrap_or_else(
                            || {
                                format!(
                                    "Tool \"{}\" was denied by permission policy.",
                                    context.tool_call.name
                                )
                            },
                        ))),
                        ..AuthorizeToolExecutionResult::default()
                    }))
                }
                PermissionPolicyResult::Ask {
                    resolve_approval,
                    resolve_error,
                    ..
                } => {
                    self.request_tool_approval(
                        context,
                        resolve_approval.as_deref(),
                        resolve_error.as_deref(),
                        policy_name,
                    )
                    .await
                }
            }
        })
    }

    fn resolve_resolution<'a>(
        &'a self,
        resolution: PermissionPolicyResolution,
        context: &'a ResolvedToolExecutionHookContext,
        policy_name: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<AuthorizeToolExecutionResult>, PermissionGateError>> {
        Box::pin(async move {
            match resolution {
                PermissionPolicyResolution::Result(result) => {
                    self.resolve_policy_result(&result, context, policy_name)
                        .await
                }
                PermissionPolicyResolution::Prepared(result) => {
                    Ok(Some(AuthorizeToolExecutionResult {
                        block: result.block,
                        reason: result.reason,
                        synthetic_result: result.synthetic_result,
                        execution_metadata: result.execution_metadata,
                    }))
                }
            }
        })
    }

    async fn request_tool_approval(
        &self,
        context: &ResolvedToolExecutionHookContext,
        resolve_approval: Option<
            &(dyn Fn(ApprovalResponse) -> Option<PermissionPolicyResolution> + Send + Sync),
        >,
        resolve_error: Option<&(dyn Fn(Value) -> Option<PermissionPolicyResolution> + Send + Sync)>,
        policy_name: Option<&str>,
    ) -> Result<Option<AuthorizeToolExecutionResult>, PermissionGateError> {
        let name = context.tool_call.name.clone();
        let action = context
            .execution
            .description
            .clone()
            .unwrap_or_else(|| format!("Approve {name}"));
        let display =
            context
                .execution
                .display
                .clone()
                .unwrap_or_else(|| ToolInputDisplay::Generic {
                    summary: action.clone(),
                    detail: kimi_code_protocol::OptionalJsonValue::Present(context.args.clone()),
                });
        let request = ApprovalRequest {
            id: None,
            session_id: Some(self.session.session_id.clone()),
            agent_id: Some(self.scope.agent_id.clone()),
            turn_id: Some(context.turn_id),
            tool_call_id: Some(context.tool_call.id.clone()),
            tool_name: name.clone(),
            action: action.clone(),
            display: display.clone(),
        };
        let approval_context = approval_event_fields(&request, &context.args);
        let started_at = Instant::now();

        let response = if let Some(approval) = &self.approval {
            self.event_bus.publish(DomainEvent::new(
                "permission.approval.requested",
                approval_context.clone(),
            ));
            match abortable(approval.request(request.clone()), &context.signal).await {
                Err(error) if error.is_user_cancellation() => {
                    return Err(PermissionGateError::Aborted(error));
                }
                Err(error) => {
                    self.publish_approval_error(
                        context,
                        policy_name,
                        &name,
                        &display,
                        started_at,
                        &approval_context,
                        &error.to_string(),
                    );
                    if let Some(resolution) =
                        resolve_error.and_then(|resolve| resolve(Value::String(error.to_string())))
                    {
                        return self
                            .resolve_resolution(resolution, context, policy_name)
                            .await;
                    }
                    return Err(PermissionGateError::Aborted(error));
                }
                Ok(Err(error)) => {
                    self.publish_approval_error(
                        context,
                        policy_name,
                        &name,
                        &display,
                        started_at,
                        &approval_context,
                        &error.to_string(),
                    );
                    if let Some(resolution) =
                        resolve_error.and_then(|resolve| resolve(Value::String(error.to_string())))
                    {
                        return self
                            .resolve_resolution(resolution, context, policy_name)
                            .await;
                    }
                    return Err(error.into());
                }
                Ok(Ok(response)) => response,
            }
        } else {
            ApprovalResponse {
                decision: ApprovalDecision::Approved,
                scope: None,
                feedback: None,
                selected_label: None,
            }
        };

        let session_approval_rule = (response.decision == ApprovalDecision::Approved
            && response.scope == Some(ApprovalScope::Session))
        .then(|| context.execution.approval_rule.clone());
        if self.approval.is_some() {
            let mut fields = approval_context.clone();
            extend_serialized_fields(&mut fields, &response);
            self.event_bus
                .publish(DomainEvent::new("permission.approval.resolved", fields));
        }
        self.rules
            .record_approval_result(PermissionApprovalResultRecord {
                turn_id: context.turn_id,
                tool_call_id: context.tool_call.id.clone(),
                tool_name: name.clone(),
                action,
                session_approval_rule: session_approval_rule.clone(),
                result: response.clone(),
            })?;
        let _ = self.telemetry.track_event(&PermissionApprovalResultEvent {
            turn_id: turn_id(context.turn_id),
            tool_call_id: context.tool_call.id.clone(),
            policy_name: policy_name.map(str::to_owned),
            tool_name: name.clone(),
            permission_mode: telemetry_mode(self.mode.mode()),
            result: telemetry_approval_result(&response),
            approval_surface: display_kind(&display),
            duration_ms: started_at.elapsed().as_millis() as u64,
            session_cache_written: session_approval_rule.is_some(),
            has_feedback: response
                .feedback
                .as_ref()
                .is_some_and(|feedback| !feedback.is_empty()),
            trace_id: context.trace.as_ref().and_then(|trace| trace.trace_id()),
        });

        if let Some(resolution) = resolve_approval.and_then(|resolve| resolve(response.clone())) {
            return self
                .resolve_resolution(resolution, context, policy_name)
                .await;
        }
        if response.decision == ApprovalDecision::Approved {
            return Ok(None);
        }
        Ok(Some(AuthorizeToolExecutionResult {
            block: Some(true),
            reason: Some(self.format_approval_rejection_message(&name, &response)),
            ..AuthorizeToolExecutionResult::default()
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn publish_approval_error(
        &self,
        context: &ResolvedToolExecutionHookContext,
        policy_name: Option<&str>,
        name: &str,
        display: &ToolInputDisplay,
        started_at: Instant,
        approval_context: &Map<String, Value>,
        error: &str,
    ) {
        let _ = self.telemetry.track_event(&PermissionApprovalResultEvent {
            turn_id: turn_id(context.turn_id),
            tool_call_id: context.tool_call.id.clone(),
            policy_name: policy_name.map(str::to_owned),
            tool_name: name.into(),
            permission_mode: telemetry_mode(self.mode.mode()),
            result: TelemetryApprovalResult::Error,
            approval_surface: display_kind(display),
            duration_ms: started_at.elapsed().as_millis() as u64,
            session_cache_written: false,
            has_feedback: false,
            trace_id: context.trace.as_ref().and_then(|trace| trace.trace_id()),
        });
        let mut fields = approval_context.clone();
        fields.insert("decision".into(), Value::String("error".into()));
        fields.insert("error".into(), Value::String(error.into()));
        self.event_bus
            .publish(DomainEvent::new("permission.approval.resolved", fields));
    }

    fn format_approval_rejection_message(
        &self,
        tool_name: &str,
        response: &ApprovalResponse,
    ) -> String {
        let suffix = response
            .feedback
            .as_ref()
            .filter(|feedback| !feedback.is_empty())
            .map_or_else(String::new, |feedback| format!(" Reason: {feedback}"));
        let prefix = if response.decision == ApprovalDecision::Cancelled {
            format!("Tool \"{tool_name}\" was not run because the approval request was cancelled.")
        } else {
            format!(
                "Tool \"{tool_name}\" was not run because the user rejected the approval request."
            )
        };
        if self.uses_worker_rejection_guidance() {
            format!(
                "{prefix}{suffix} Try a different approach — don't retry the same call, don't attempt to bypass the restriction."
            )
        } else {
            format!("{prefix}{suffix}")
        }
    }

    fn format_deny_message(&self, message: impl AsRef<str>) -> String {
        let message = message.as_ref();
        if self.uses_worker_rejection_guidance() {
            format!(
                "{message} Try a different approach — don't retry the same call, don't attempt to bypass the restriction."
            )
        } else {
            message.into()
        }
    }

    fn uses_worker_rejection_guidance(&self) -> bool {
        self.scope.agent_id != "main"
    }
}

impl AgentPermissionGateContract for AgentPermissionGate {
    fn data(&self) -> PermissionData {
        PermissionData {
            mode: self.mode.mode(),
            rules: self.rules.rules(),
        }
    }

    fn authorize<'a>(
        &'a self,
        context: &'a ResolvedToolExecutionHookContext,
    ) -> BoxFuture<'a, Result<Option<AuthorizeToolExecutionResult>, PermissionGateError>> {
        Box::pin(async move {
            let Some(evaluation) = self.policy.evaluate(context).await else {
                return Ok(None);
            };
            let _ = self.telemetry.track_event(&PermissionPolicyDecisionEvent {
                turn_id: turn_id(context.turn_id),
                tool_call_id: context.tool_call.id.clone(),
                policy_name: evaluation.policy_name.clone(),
                tool_name: context.tool_call.name.clone(),
                permission_mode: telemetry_mode(self.mode.mode()),
                decision: telemetry_decision(evaluation.result.as_ref()),
                extra: reason_properties(evaluation.result.as_ref()),
            });
            self.resolve_policy_result(
                evaluation.result.as_ref(),
                context,
                Some(&evaluation.policy_name),
            )
            .await
        })
    }
}

impl Disposable for AgentPermissionGate {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

pub fn register_agent_permission_gate() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_PERMISSION_GATE_ID,
        SyncDescriptor::new(|accessor| {
            let scope = (*accessor.get(AGENT_SCOPE_CONTEXT_ID)?).clone();
            let mode: AgentPermissionModeServiceHandle =
                (*accessor.get(AGENT_PERMISSION_MODE_SERVICE_ID)?).clone();
            let rules: AgentPermissionRulesServiceHandle =
                (*accessor.get(AGENT_PERMISSION_RULES_SERVICE_ID)?).clone();
            let policy: AgentPermissionPolicyServiceHandle =
                (*accessor.get(AGENT_PERMISSION_POLICY_SERVICE_ID)?).clone();
            let session = (*accessor.get(SESSION_CONTEXT_ID)?).clone();
            let approval = Some((*accessor.get(SESSION_APPROVAL_SERVICE_ID)?).clone());
            let telemetry = (*accessor.get(TELEMETRY_SERVICE_ID)?).clone();
            let event_bus = (*accessor.get(EVENT_BUS_SERVICE_ID)?).clone();
            let executor = (*accessor.get(AGENT_TOOL_EXECUTOR_SERVICE_ID)?).clone();
            let service = AgentPermissionGate::new(
                scope, mode.0, rules.0, policy, session, approval, telemetry, event_bus, executor,
            )
            .map_err(|error| crate::_base::di::errors::DiError::Factory(error.to_string()))?;
            let service: Arc<dyn AgentPermissionGateContract> = service;
            Ok(AgentPermissionGateHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "permissionGate",
    );
}

fn approval_event_fields(request: &ApprovalRequest, tool_input: &Value) -> Map<String, Value> {
    let mut fields = serde_json::to_value(request)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    fields.insert("toolInput".into(), tool_input.clone());
    fields
}

fn extend_serialized_fields(fields: &mut Map<String, Value>, value: &impl Serialize) {
    if let Ok(Value::Object(extra)) = serde_json::to_value(value) {
        fields.extend(extra);
    }
}

fn display_kind(display: &ToolInputDisplay) -> String {
    serde_json::to_value(display)
        .ok()
        .and_then(|value| value.get("kind").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| "generic".into())
}

fn turn_id(value: crate::agent::TurnId) -> crate::agent::TurnId {
    value
}

fn telemetry_mode(mode: PermissionMode) -> TelemetryPermissionMode {
    match mode {
        PermissionMode::Manual => TelemetryPermissionMode::Manual,
        PermissionMode::Yolo => TelemetryPermissionMode::Yolo,
        PermissionMode::Auto => TelemetryPermissionMode::Auto,
    }
}

fn telemetry_decision(result: &PermissionPolicyResult) -> TelemetryPermissionDecision {
    match result {
        PermissionPolicyResult::Approve { .. } => TelemetryPermissionDecision::Approve,
        PermissionPolicyResult::Deny { .. } => TelemetryPermissionDecision::Deny,
        PermissionPolicyResult::Ask { .. } => TelemetryPermissionDecision::Ask,
    }
}

fn telemetry_approval_result(response: &ApprovalResponse) -> TelemetryApprovalResult {
    match (response.decision, response.scope) {
        (ApprovalDecision::Approved, Some(ApprovalScope::Session)) => {
            TelemetryApprovalResult::ApprovedForSession
        }
        (ApprovalDecision::Approved, _) => TelemetryApprovalResult::Approved,
        (ApprovalDecision::Rejected, _) => TelemetryApprovalResult::Rejected,
        (ApprovalDecision::Cancelled, _) => TelemetryApprovalResult::Cancelled,
    }
}

fn reason_properties(result: &PermissionPolicyResult) -> IndexMap<String, Value> {
    let reason: Option<&PermissionDecisionReason> = match result {
        PermissionPolicyResult::Approve { reason, .. }
        | PermissionPolicyResult::Deny { reason, .. }
        | PermissionPolicyResult::Ask { reason, .. } => reason.as_ref(),
    };
    reason
        .map(|reason| {
            reason
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        serde_json::to_value(value).unwrap_or(Value::Null),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures_util::{FutureExt, StreamExt, stream};
    use serde_json::json;
    use parking_lot::Mutex;

    use crate::{
        _base::{
            di::lifecycle::{DisposableHandle, disposable_none},
            event::{Emitter, Event},
            utils::abort::{AbortController, user_cancellation_reason},
        },
        agent::{
            permission_mode::{PermissionModeChangedContext, PermissionModeServiceError},
            permission_policy::{PermissionPolicy, PermissionPolicyFuture},
            permission_rules::{PermissionRuleDecision, PermissionRuleScope},
            tool_executor::{
                AgentToolExecutorHooks, AgentToolExecutorServiceContract, MissingToolDescriber,
                ToolCallDupType, ToolCallGuard, ToolExecutionStream, ToolExecutorExecuteOptions,
                UnavailableToolDescriber,
            },
        },
        app::{
            event::{event_bus::EventBusContract, event_bus_service::EventBusService},
            telemetry::noop_telemetry_service,
        },
        kosong::contract::message::{ToolCall, ToolCallType},
        session::session_context::{SessionContextInput, make_session_context},
        tool::{ExecutableToolResult, RunnableToolExecution},
    };

    struct Mode(PermissionMode);

    impl AgentPermissionModeServiceContract for Mode {
        fn mode(&self) -> PermissionMode {
            self.0
        }
        fn set_mode(&self, _: PermissionMode) -> Result<(), PermissionModeServiceError> {
            Ok(())
        }
        fn on_did_change_mode(&self) -> Event<PermissionModeChangedContext> {
            Arc::new(Emitter::new()).event()
        }
    }

    impl Disposable for Mode {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    #[derive(Default)]
    struct Rules {
        records: Mutex<Vec<PermissionApprovalResultRecord>>,
    }

    impl AgentPermissionRulesServiceContract for Rules {
        fn rules(&self) -> Vec<PermissionRule> {
            vec![PermissionRule {
                decision: PermissionRuleDecision::Ask,
                scope: PermissionRuleScope::User,
                pattern: "Bash(*)".into(),
                reason: None,
            }]
        }
        fn session_approval_rule_patterns(&self) -> Vec<String> {
            self.records
                .lock()
                .iter()
                .filter_map(|record| record.session_approval_rule.clone())
                .collect()
        }
        fn add_rules(
            &self,
            _: &[PermissionRule],
        ) -> Result<(), crate::agent::permission_rules::PermissionRulesServiceError> {
            Ok(())
        }
        fn record_approval_result(
            &self,
            record: PermissionApprovalResultRecord,
        ) -> Result<(), crate::agent::permission_rules::PermissionRulesServiceError> {
            self.records.lock().push(record);
            Ok(())
        }
    }

    enum StaticDecision {
        Ask,
        Deny,
    }

    struct StaticPolicy(StaticDecision);

    impl PermissionPolicy for StaticPolicy {
        fn name(&self) -> &str {
            "test-policy"
        }

        fn evaluate<'a>(
            &'a self,
            _: &'a ResolvedToolExecutionHookContext,
        ) -> PermissionPolicyFuture<'a> {
            async move {
                Some(match self.0 {
                    StaticDecision::Ask => PermissionPolicyResult::Ask {
                        reason: None,
                        resolve_approval: None,
                        resolve_error: None,
                    },
                    StaticDecision::Deny => PermissionPolicyResult::Deny {
                        reason: None,
                        message: None,
                    },
                })
            }
            .boxed()
        }
    }

    struct Approval {
        response: ApprovalResponse,
        requests: Mutex<Vec<ApprovalRequest>>,
    }

    #[async_trait]
    impl crate::session::approval::SessionApprovalServiceContract for Approval {
        async fn request(
            &self,
            request: ApprovalRequest,
        ) -> Result<ApprovalResponse, serde_json::Error> {
            self.requests.lock().push(request);
            Ok(self.response.clone())
        }
        async fn enqueue(&self, request: ApprovalRequest) -> ApprovalRequest {
            request
        }
        async fn decide(&self, _: &str, _: ApprovalResponse) {}
        async fn list_pending(&self) -> Vec<ApprovalRequest> {
            self.requests.lock().clone()
        }
    }

    struct PendingApproval;

    #[async_trait]
    impl crate::session::approval::SessionApprovalServiceContract for PendingApproval {
        async fn request(&self, _: ApprovalRequest) -> Result<ApprovalResponse, serde_json::Error> {
            std::future::pending().await
        }
        async fn enqueue(&self, request: ApprovalRequest) -> ApprovalRequest {
            request
        }
        async fn decide(&self, _: &str, _: ApprovalResponse) {}
        async fn list_pending(&self) -> Vec<ApprovalRequest> {
            Vec::new()
        }
    }

    #[derive(Default)]
    struct Executor {
        hooks: AgentToolExecutorHooks,
    }

    impl AgentToolExecutorServiceContract for Executor {
        fn execute(&self, _: Vec<ToolCall>, _: ToolExecutorExecuteOptions) -> ToolExecutionStream {
            stream::empty().boxed()
        }
        fn hooks(&self) -> &AgentToolExecutorHooks {
            &self.hooks
        }
        fn record_dup_type(&self, _: String, _: ToolCallDupType) {}
        fn register_tool_call_guard(&self, _: ToolCallGuard) -> DisposableHandle {
            disposable_none()
        }
        fn register_unavailable_tool_describer(
            &self,
            _: UnavailableToolDescriber,
        ) -> DisposableHandle {
            disposable_none()
        }
        fn register_missing_tool_describer(&self, _: MissingToolDescriber) -> DisposableHandle {
            disposable_none()
        }
    }

    fn context(
        signal: crate::_base::utils::abort::AbortSignal,
    ) -> ResolvedToolExecutionHookContext {
        let execute = Arc::new(|_| {
            Box::pin(async { ExecutableToolResult::success("ran") })
                as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new("Bash(git status)", execute);
        execution.description = Some("Running: git status".into());
        execution.display = Some(ToolInputDisplay::Command {
            command: "git status".into(),
            cwd: Some("/workspace".into()),
            description: None,
            language: Some(kimi_code_protocol::CommandLanguage::Bash),
        });
        ResolvedToolExecutionHookContext::new(
            crate::agent::tool_executor::ToolExecutionHookContext {
                turn_id: crate::agent::TurnId::new(1),
                signal,
                trace: None,
                tool_call: ToolCall {
                    call_type: ToolCallType::Function,
                    id: "call-Bash".into(),
                    name: "Bash".into(),
                    arguments: Some(r#"{"command":"git status"}"#.into()),
                    extras: None,
                    stream_index: None,
                },
                tool_calls: Vec::new(),
                tool: None,
                args: json!({"command":"git status"}),
            },
            execution,
        )
    }

    fn gate(
        decision: StaticDecision,
        response: ApprovalResponse,
    ) -> (
        Arc<AgentPermissionGate>,
        Arc<Rules>,
        Arc<Approval>,
        Arc<EventBusService>,
    ) {
        let mode: Arc<dyn AgentPermissionModeServiceContract> =
            Arc::new(Mode(PermissionMode::Manual));
        let rules = Arc::new(Rules::default());
        let rules_contract: Arc<dyn AgentPermissionRulesServiceContract> = rules.clone();
        let policy: Arc<dyn crate::agent::permission_policy::AgentPermissionPolicyServiceContract> =
            Arc::new(
                crate::agent::permission_policy::AgentPermissionPolicyService::new(vec![Arc::new(
                    StaticPolicy(decision),
                )]),
            );
        let approval = Arc::new(Approval {
            response,
            requests: Mutex::new(Vec::new()),
        });
        let approval_contract: Arc<dyn crate::session::approval::SessionApprovalServiceContract> =
            approval.clone();
        let bus = Arc::new(EventBusService::new());
        let bus_contract: Arc<dyn EventBusContract> = bus.clone();
        let executor: Arc<dyn AgentToolExecutorServiceContract> = Arc::new(Executor::default());
        let gate = AgentPermissionGate::new(
            crate::agent::scope_context::make_agent_scope_context(
                crate::agent::scope_context::AgentScopeContextInput {
                    agent_id: "main".into(),
                    agent_scope: "agents/main".into(),
                },
            ),
            mode,
            rules_contract,
            AgentPermissionPolicyServiceHandle(policy),
            make_session_context(SessionContextInput {
                session_id: "session".into(),
                workspace_id: "workspace".into(),
                session_dir: "/session".into(),
                session_scope: "sessions/workspace/session".into(),
                cwd: "/workspace".into(),
                meta_scope: None,
            }),
            Some(SessionApprovalServiceHandle(approval_contract)),
            noop_telemetry_service(),
            EventBusHandle(bus_contract),
            AgentToolExecutorServiceHandle(executor),
        )
        .unwrap();
        (gate, rules, approval, bus)
    }

    #[test]
    fn rejection_messages_match_main_and_worker_guidance() {
        let response = ApprovalResponse {
            decision: ApprovalDecision::Rejected,
            scope: None,
            feedback: Some("unsafe".into()),
            selected_label: None,
        };
        let main = format_rejection_for("main", "Bash", &response);
        assert_eq!(
            main,
            "Tool \"Bash\" was not run because the user rejected the approval request. Reason: unsafe"
        );
        let worker = format_rejection_for("worker", "Bash", &response);
        assert!(worker.contains("Try a different approach"));
    }

    fn format_rejection_for(
        agent_id: &str,
        tool_name: &str,
        response: &ApprovalResponse,
    ) -> String {
        let suffix = response
            .feedback
            .as_ref()
            .filter(|feedback| !feedback.is_empty())
            .map_or_else(String::new, |feedback| format!(" Reason: {feedback}"));
        let prefix = format!(
            "Tool \"{tool_name}\" was not run because the user rejected the approval request."
        );
        if agent_id == "main" {
            format!("{prefix}{suffix}")
        } else {
            format!(
                "{prefix}{suffix} Try a different approach — don't retry the same call, don't attempt to bypass the restriction."
            )
        }
    }

    #[test]
    fn event_fields_and_telemetry_mappings_preserve_source_shapes() {
        let display = ToolInputDisplay::Generic {
            summary: "run".into(),
            detail: kimi_code_protocol::OptionalJsonValue::Present(json!({"command":"pwd"})),
        };
        assert_eq!(display_kind(&display), "generic");
        assert_eq!(
            telemetry_mode(PermissionMode::Manual),
            TelemetryPermissionMode::Manual
        );
        assert_eq!(
            telemetry_approval_result(&ApprovalResponse {
                decision: ApprovalDecision::Approved,
                scope: Some(ApprovalScope::Session),
                feedback: None,
                selected_label: None,
            }),
            TelemetryApprovalResult::ApprovedForSession
        );
    }

    #[tokio::test]
    async fn approved_session_request_publishes_events_and_records_rule() {
        let (gate, rules, approval, bus) = gate(
            StaticDecision::Ask,
            ApprovalResponse {
                decision: ApprovalDecision::Approved,
                scope: Some(ApprovalScope::Session),
                feedback: None,
                selected_label: Some("Approve for this session".into()),
            },
        );
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        let _subscription = bus.subscribe(Arc::new(move |event| {
            captured.lock().push(event.clone());
        }));
        let result = gate
            .authorize(&context(AbortController::new().signal()))
            .await
            .unwrap();
        assert_eq!(result, None);
        assert_eq!(approval.requests.lock().len(), 1);
        assert_eq!(
            rules.records.lock()[0]
                .session_approval_rule
                .as_deref(),
            Some("Bash(git status)")
        );
        let events = events.lock();
        assert_eq!(events[0].event_type, "permission.approval.requested");
        assert_eq!(events[0].fields["toolInput"]["command"], "git status");
        assert_eq!(events[1].event_type, "permission.approval.resolved");
        assert_eq!(events[1].fields["decision"], "approved");
    }

    #[tokio::test]
    async fn rejected_request_blocks_and_deny_uses_tool_specific_default() {
        let (ask_gate, _, _, _) = gate(
            StaticDecision::Ask,
            ApprovalResponse {
                decision: ApprovalDecision::Rejected,
                scope: None,
                feedback: Some("unsafe".into()),
                selected_label: None,
            },
        );
        let result = ask_gate
            .authorize(&context(AbortController::new().signal()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.block, Some(true));
        assert_eq!(
            result.reason.as_deref(),
            Some(
                "Tool \"Bash\" was not run because the user rejected the approval request. Reason: unsafe"
            )
        );

        let (gate, _, _, _) = gate(
            StaticDecision::Deny,
            ApprovalResponse {
                decision: ApprovalDecision::Approved,
                scope: None,
                feedback: None,
                selected_label: None,
            },
        );
        let result = gate
            .authorize(&context(AbortController::new().signal()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result.reason.as_deref(),
            Some("Tool \"Bash\" was denied by permission policy.")
        );
    }

    #[tokio::test]
    async fn user_cancellation_interrupts_a_pending_approval() {
        let mode: Arc<dyn AgentPermissionModeServiceContract> =
            Arc::new(Mode(PermissionMode::Manual));
        let rules: Arc<dyn AgentPermissionRulesServiceContract> = Arc::new(Rules::default());
        let policy: Arc<dyn crate::agent::permission_policy::AgentPermissionPolicyServiceContract> =
            Arc::new(
                crate::agent::permission_policy::AgentPermissionPolicyService::new(vec![Arc::new(
                    StaticPolicy(StaticDecision::Ask),
                )]),
            );
        let approval: Arc<dyn crate::session::approval::SessionApprovalServiceContract> =
            Arc::new(PendingApproval);
        let bus: Arc<dyn EventBusContract> = Arc::new(EventBusService::new());
        let executor: Arc<dyn AgentToolExecutorServiceContract> = Arc::new(Executor::default());
        let gate = AgentPermissionGate::new(
            crate::agent::scope_context::make_agent_scope_context(
                crate::agent::scope_context::AgentScopeContextInput {
                    agent_id: "main".into(),
                    agent_scope: "agents/main".into(),
                },
            ),
            mode,
            rules,
            AgentPermissionPolicyServiceHandle(policy),
            make_session_context(SessionContextInput {
                session_id: "session".into(),
                workspace_id: "workspace".into(),
                session_dir: "/session".into(),
                session_scope: "sessions/workspace/session".into(),
                cwd: "/workspace".into(),
                meta_scope: None,
            }),
            Some(SessionApprovalServiceHandle(approval)),
            noop_telemetry_service(),
            EventBusHandle(bus),
            AgentToolExecutorServiceHandle(executor),
        )
        .unwrap();
        let controller = AbortController::new();
        let execution = context(controller.signal());
        let pending = gate.authorize(&execution);
        tokio::pin!(pending);
        tokio::select! {
            _ = &mut pending => panic!("approval unexpectedly completed"),
            _ = tokio::task::yield_now() => {}
        }
        controller.abort(Some(user_cancellation_reason()));
        let error = pending.await.unwrap_err();
        assert!(matches!(
            error,
            PermissionGateError::Aborted(reason) if reason.is_user_cancellation()
        ));
    }
}
