//! Caller-side subagent-run event projection.
//!
//! Original: `session/subagent/mirrorAgentRun.ts`, `emitAgentRunSpawned()` and
//! `mirrorAgentRun()`.

use std::sync::Arc;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    _base::{
        di::{errors::DiError, scope::ScopeHandle},
        lifecycle::lifecycle_machine::BoxError,
        utils::abort::{AbortSignal, is_abort_error},
    },
    agent::context_size::AGENT_CONTEXT_SIZE_SERVICE_ID,
    app::{
        event::event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID},
        telemetry::TELEMETRY_SERVICE_ID,
    },
    kosong::contract::errors::{ChatProviderError, is_provider_rate_limit_error},
    session::agent_lifecycle::AGENT_LIFECYCLE_SERVICE_ID,
};

use super::{
    AgentRunCompletion, AgentRunHandle, AgentTaskStartHookContext, AgentTaskStopHookContext,
    SESSION_SUBAGENT_SERVICE_ID,
};

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunSpawnedMeta {
    pub profile_name: String,
    pub parent_tool_call_id: Option<String>,
    pub parent_tool_call_uuid: Option<String>,
    pub description: Option<String>,
    pub swarm_index: Option<u64>,
    pub run_in_background: Option<bool>,
}

#[derive(Clone)]
pub struct MirrorAgentRunOptions {
    pub profile_name: String,
    pub prompt: Option<String>,
    pub suppress_rate_limit_failure_event: bool,
    pub signal: AbortSignal,
    pub cancel: Option<Arc<dyn Fn(Option<BoxError>) + Send + Sync>>,
}

pub fn emit_agent_run_spawned(
    requester: &ScopeHandle,
    target_agent_id: &str,
    meta: &AgentRunSpawnedMeta,
) {
    let run_in_background = meta.run_in_background.unwrap_or(false);
    if let Ok(bus) = requester.get(EVENT_BUS_SERVICE_ID) {
        bus.publish(DomainEvent::new(
            "subagent.spawned",
            serde_json::Map::from_iter([
                ("subagentId".into(), Value::String(target_agent_id.into())),
                (
                    "subagentName".into(),
                    Value::String(meta.profile_name.clone()),
                ),
                (
                    "parentToolCallId".into(),
                    Value::String(meta.parent_tool_call_id.clone().unwrap_or_default()),
                ),
                ("parentAgentId".into(), Value::String(requester.id().into())),
                ("callerAgentId".into(), Value::String(requester.id().into())),
                ("runInBackground".into(), Value::Bool(run_in_background)),
            ]),
        ));
    }
    if let Ok(telemetry) = requester.get(TELEMETRY_SERVICE_ID) {
        telemetry.track(
            "subagent_created",
            Some(&IndexMap::from([
                (
                    "subagent_name".into(),
                    Some(Value::String(meta.profile_name.clone())),
                ),
                (
                    "run_in_background".into(),
                    Some(Value::Bool(run_in_background)),
                ),
                (
                    "agent_id".into(),
                    Some(Value::String(target_agent_id.into())),
                ),
                (
                    "parent_agent_id".into(),
                    Some(Value::String(requester.id().into())),
                ),
                (
                    "parent_tool_call_id".into(),
                    Some(Value::String(
                        meta.parent_tool_call_id.clone().unwrap_or_default(),
                    )),
                ),
            ])),
        );
    }
}

pub async fn mirror_agent_run(
    requester: &ScopeHandle,
    run: AgentRunHandle,
    options: MirrorAgentRunOptions,
) -> Result<AgentRunCompletion, BoxError> {
    let bus = optional(requester, EVENT_BUS_SERVICE_ID)?;
    let subagents = optional(requester, SESSION_SUBAGENT_SERVICE_ID)?;
    let lifecycle = optional(requester, AGENT_LIFECYCLE_SERVICE_ID)?;
    publish(
        &bus,
        "subagent.started",
        serde_json::Map::from_iter([("subagentId".into(), Value::String(run.agent_id.clone()))]),
    );
    if let Some(prompt) = options.prompt.clone() {
        if let Some(subagents) = &subagents {
            let mut context = AgentTaskStartHookContext {
                agent_name: options.profile_name.clone(),
                prompt,
                signal: options.signal.clone(),
            };
            if let Err(error) = subagents
                .hooks()
                .on_will_start_agent_task
                .run(&mut context, None)
                .await
            {
                cancel_and_rethrow(&run, &options, error).await?;
            }
        }
        if let Some(reason) = options.signal.reason() {
            cancel_and_rethrow(&run, &options, Box::new((*reason).clone())).await?;
        }
    }
    match run.completion.await {
        Ok(result) => {
            let context_tokens = lifecycle
                .and_then(|lifecycle| lifecycle.get(&run.agent_id))
                .and_then(|child| child.get(AGENT_CONTEXT_SIZE_SERVICE_ID).ok())
                .map(|size| size.get(None, None).size);
            let mut fields = serde_json::Map::from_iter([
                ("subagentId".into(), Value::String(run.agent_id.clone())),
                (
                    "resultSummary".into(),
                    Value::String(result.summary.clone()),
                ),
            ]);
            if let Some(usage) = result.usage {
                fields.insert("usage".into(), serde_json::to_value(usage)?);
            }
            if let Some(tokens) = context_tokens {
                fields.insert("contextTokens".into(), Value::from(tokens));
            }
            publish(&bus, "subagent.completed", fields);
            if let Some(subagents) = subagents {
                subagents.notify_agent_task_stopped(AgentTaskStopHookContext {
                    agent_name: options.profile_name,
                    response: result.summary.clone(),
                });
            }
            Ok(result)
        }
        Err(error) => {
            let error: BoxError = Box::new(super::SharedAgentRunError(error));
            if !is_abort_error(error.as_ref()) && !should_suppress_failure(&options, error.as_ref())
            {
                publish(
                    &bus,
                    "subagent.failed",
                    serde_json::Map::from_iter([
                        ("subagentId".into(), Value::String(run.agent_id)),
                        ("error".into(), Value::String(error.to_string())),
                    ]),
                );
            }
            Err(error)
        }
    }
}

async fn cancel_and_rethrow(
    run: &AgentRunHandle,
    options: &MirrorAgentRunOptions,
    error: BoxError,
) -> Result<(), BoxError> {
    if let Some(cancel) = &options.cancel {
        cancel(None);
    }
    let _ = run.completion.clone().await;
    Err(error)
}

fn optional<T: Send + Sync + 'static>(
    scope: &ScopeHandle,
    id: crate::_base::di::instantiation::ServiceIdentifier<T>,
) -> Result<Option<Arc<T>>, BoxError> {
    match scope.get(id) {
        Ok(value) => Ok(Some(value)),
        Err(DiError::UnknownService(_)) => Ok(None),
        Err(error) => Err(Box::new(error)),
    }
}
fn publish(
    bus: &Option<Arc<crate::app::event::event_bus::EventBusHandle>>,
    event_type: &str,
    fields: serde_json::Map<String, Value>,
) {
    if let Some(bus) = bus {
        bus.publish(DomainEvent::new(event_type, fields));
    }
}
fn should_suppress_failure(
    options: &MirrorAgentRunOptions,
    error: &(dyn std::error::Error + 'static),
) -> bool {
    options.suppress_rate_limit_failure_event
        && (error
            .downcast_ref::<ChatProviderError>()
            .is_some_and(is_provider_rate_limit_error)
            || is_abort_error(error)
            || options.signal.aborted())
}
