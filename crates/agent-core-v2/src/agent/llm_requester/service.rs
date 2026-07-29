//! Agent LLM requester implementation.
//!
//! Original: `packages/agent-core-v2/src/agent/llmRequester/llmRequesterService.ts`.

use std::{
    collections::HashMap,
    error::Error,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use futures_util::{StreamExt, future::FutureExt};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        utils::abort::AbortSignal,
    },
    agent::{
        context_memory::{AGENT_CONTEXT_MEMORY_SERVICE_ID, AgentContextMemoryServiceHandle},
        context_projector::{
            AGENT_CONTEXT_PROJECTOR_SERVICE_ID, AgentContextProjectorServiceHandle,
        },
        context_size::{AGENT_CONTEXT_SIZE_SERVICE_ID, AgentContextSizeServiceHandle},
        fault_injection::{FAULT_INJECTION_SERVICE_ID, FaultInjectionServiceHandle, FaultKind},
        profile::{AGENT_PROFILE_SERVICE_ID, AgentProfileServiceHandle, ProfileModelContext},
        tool_registry::{AGENT_TOOL_REGISTRY_SERVICE_ID, AgentToolRegistryServiceHandle},
        tool_select::{AGENT_TOOL_SELECT_SERVICE_ID, AgentToolSelectServiceHandle},
        usage::{AGENT_USAGE_SERVICE_ID, AgentUsageServiceHandle},
    },
    kosong::{
        contract::{
            errors::{ApiStatusData, ChatProviderError},
            request_trace::LlmRequestTrace,
            tool::Tool,
            usage::empty_usage,
        },
        model::{
            MODEL_CATALOG_SERVICE_ID, ModelCatalogHandle, ModelRequestEvent, ModelRequestInput,
            ModelRequestParams,
        },
    },
};

use super::{
    AGENT_LLM_REQUESTER_SERVICE_ID, AgentLlmRequestError, AgentLlmRequestFinish,
    AgentLlmRequestOverrides, AgentLlmRequestPartHandler, AgentLlmRequestSource,
    AgentLlmRequestTask, AgentLlmRequesterServiceContract, AgentLlmRequesterServiceHandle,
    PreparedTurnRequestConfig,
};

type TurnRequestConfig = (ProfileModelContext, ModelRequestParams, String);

#[derive(Default)]
struct TurnConfigCache {
    configs: Arc<Mutex<HashMap<i64, TurnRequestConfig>>>,
}

impl TurnConfigCache {
    fn clone_for_task(&self) -> Self {
        Self {
            configs: Arc::clone(&self.configs),
        }
    }

    fn get(&self, id: i64) -> Option<TurnRequestConfig> {
        self.configs.lock().unwrap().get(&id).cloned()
    }

    fn insert(&self, id: i64, config: TurnRequestConfig) {
        let mut configs = self.configs.lock().unwrap();
        configs.retain(|key, _| *key >= id);
        configs.insert(id, config);
    }
}

struct AbortSignalBridge {
    token: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl AbortSignalBridge {
    fn new(signal: AbortSignal) -> Self {
        let token = CancellationToken::new();
        if signal.aborted() {
            token.cancel();
            return Self { token, task: None };
        }
        let linked_token = token.clone();
        let task = tokio::spawn(async move {
            signal.cancelled().await;
            linked_token.cancel();
        });
        Self {
            token,
            task: Some(task),
        }
    }
}

impl Drop for AbortSignalBridge {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub struct AgentLlmRequesterService {
    context: AgentContextMemoryServiceHandle,
    projector: AgentContextProjectorServiceHandle,
    context_size: AgentContextSizeServiceHandle,
    tools: AgentToolRegistryServiceHandle,
    tool_select: AgentToolSelectServiceHandle,
    profile: AgentProfileServiceHandle,
    usage: AgentUsageServiceHandle,
    catalog: ModelCatalogHandle,
    fault: FaultInjectionServiceHandle,
    turn_configs: TurnConfigCache,
}

impl AgentLlmRequesterService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: AgentContextMemoryServiceHandle,
        projector: AgentContextProjectorServiceHandle,
        context_size: AgentContextSizeServiceHandle,
        tools: AgentToolRegistryServiceHandle,
        tool_select: AgentToolSelectServiceHandle,
        profile: AgentProfileServiceHandle,
        usage: AgentUsageServiceHandle,
        catalog: ModelCatalogHandle,
        fault: FaultInjectionServiceHandle,
    ) -> Self {
        Self {
            context,
            projector,
            context_size,
            tools,
            tool_select,
            profile,
            usage,
            catalog,
            fault,
            turn_configs: TurnConfigCache::default(),
        }
    }

    fn turn_config(&self, id: i64) -> Result<TurnRequestConfig, AgentLlmRequestError> {
        if let Some(config) = self.turn_configs.get(id) {
            return Ok(config);
        }
        let config = (
            self.profile
                .resolve_model_context()
                .map_err(AgentLlmRequestError::from)?,
            self.profile
                .resolve_request_params()
                .map_err(AgentLlmRequestError::from)?,
            self.profile.get_system_prompt(),
        );
        self.turn_configs.insert(id, config.clone());
        Ok(config)
    }

    fn default_tools(&self) -> Vec<Tool> {
        self.tool_select
            .shape_tools(&self.tools.list())
            .into_iter()
            .map(|entry| Tool {
                name: entry.info.name,
                description: entry.info.description,
                parameters: entry.info.parameters.unwrap_or_else(|| {
                    serde_json::Map::from_iter([
                        ("type".into(), serde_json::Value::String("object".into())),
                        (
                            "properties".into(),
                            serde_json::Value::Object(Default::default()),
                        ),
                    ])
                }),
                deferred: entry.deferred.then_some(true),
            })
            .collect()
    }

    async fn perform(
        &self,
        overrides: AgentLlmRequestOverrides,
        on_part: Option<AgentLlmRequestPartHandler>,
        signal: Option<AbortSignal>,
        trace: LlmRequestTrace,
    ) -> Result<AgentLlmRequestFinish, AgentLlmRequestError> {
        if let Some(signal) = &signal {
            signal
                .throw_if_aborted()
                .map_err(|error| error as AgentLlmRequestError)?;
        }
        let source_turn = match &overrides.source {
            Some(AgentLlmRequestSource::Turn { turn_id, .. }) => Some(*turn_id as i64),
            _ => None,
        };
        let (resolved, params, prompt) = match source_turn {
            Some(id) => self.turn_config(id)?,
            None => (
                self.profile
                    .resolve_model_context()
                    .map_err(AgentLlmRequestError::from)?,
                self.profile
                    .resolve_request_params()
                    .map_err(AgentLlmRequestError::from)?,
                self.profile.get_system_prompt(),
            ),
        };
        let requester = self
            .catalog
            .get_requester(&resolved.model_alias)
            .map_err(AgentLlmRequestError::from)?;
        let history = overrides
            .messages
            .as_ref()
            .map(|messages| {
                messages
                    .iter()
                    .cloned()
                    .map(|message| crate::agent::context_memory::ContextMessage {
                        message,
                        id: None,
                        provider_message_id: None,
                        origin: None,
                        is_error: None,
                        note: None,
                    })
                    .collect()
            })
            .unwrap_or_else(|| self.context.get());
        let shaped = self.tool_select.shape_history(&history);
        let tools = overrides.tools.unwrap_or_else(|| self.default_tools());
        let system_prompt = overrides.system_prompt.unwrap_or(prompt);
        let mut projection = 0_u8;
        loop {
            trace.set_trace_id(None);
            let messages = match projection {
                0 => self.projector.project(&shaped),
                1 => self.projector.project_strict(&shaped),
                2 => self.projector.project_media_degraded(&shaped),
                _ => self.projector.project_media_stripped(&shaped, None),
            }
            .map_err(|error| Arc::new(error) as AgentLlmRequestError)?;
            match self
                .run_stream(
                    requester.clone(),
                    ModelRequestInput {
                        system_prompt: system_prompt.clone(),
                        tools: tools.clone(),
                        messages: messages.clone(),
                        response_format: None,
                    },
                    params.clone(),
                    on_part.clone(),
                    signal.clone(),
                    trace.clone(),
                )
                .await
            {
                Ok(finish) => {
                    let _ = self.usage.record(
                        resolved.model_alias,
                        finish.usage,
                        overrides.source.clone(),
                    );
                    let _ = self.context_size.measured(
                        &messages,
                        std::slice::from_ref(&finish.message),
                        finish.usage,
                    );
                    return Ok(finish);
                }
                Err(error) => {
                    if signal.as_ref().is_some_and(AbortSignal::aborted) {
                        return Err(error);
                    }
                    let Some(provider_error) = find_provider_error(error.as_ref()) else {
                        return Err(error);
                    };
                    projection = match (provider_error, projection) { (ChatProviderError::ApiRequestTooLarge { .. }, 0) => 2, (ChatProviderError::ApiRequestTooLarge { .. }, 2) => 3, (_, 0) if crate::kosong::contract::errors::is_image_format_error(provider_error) => 3, (_, 0) if crate::kosong::contract::errors::is_recoverable_request_structure_error(provider_error) => 1, _ => return Err(error) };
                }
            }
        }
    }

    async fn run_stream(
        &self,
        requester: Arc<dyn crate::kosong::model::ModelRequester>,
        input: ModelRequestInput,
        params: ModelRequestParams,
        on_part: Option<AgentLlmRequestPartHandler>,
        signal: Option<AbortSignal>,
        trace: LlmRequestTrace,
    ) -> Result<AgentLlmRequestFinish, AgentLlmRequestError> {
        if let Some(fault) = self.fault.take() {
            let error = match fault {
                FaultKind::RequestTooLarge => ChatProviderError::ApiRequestTooLarge {
                    message: "Request Entity Too Large (fault injection)".into(),
                    data: ApiStatusData::new(413, None, None, None),
                },
                FaultKind::ImageFormat => ChatProviderError::ApiStatus {
                    message: "unsupported image format: image/avif (fault injection)".into(),
                    data: ApiStatusData::new(400, None, None, None),
                },
            };
            return Err(Arc::new(error));
        }
        let signal_bridge = signal.map(AbortSignalBridge::new);
        let trace_for_callback = trace.clone();
        let mut params = params;
        params.on_trace_id = Some(Arc::new(move |trace_id| {
            trace_for_callback.set_trace_id(trace_id.map(str::to_owned));
        }));
        let mut stream = requester.request(
            input,
            signal_bridge.as_ref().map(|bridge| bridge.token.clone()),
            Some(params),
        );
        let mut usage = empty_usage();
        let mut timing = None;
        let mut finish = None;
        while let Some(event) = stream.next().await {
            match event.map_err(|error| -> AgentLlmRequestError {
                match error {
                    crate::kosong::model::ModelRequestError::Abort(error) => Arc::new(error),
                    crate::kosong::model::ModelRequestError::Coded(error) => Arc::new(error),
                }
            })? {
                ModelRequestEvent::Part(part) => {
                    if let Some(handler) = &on_part {
                        handler(part).await.map_err(|message| {
                            Arc::new(ChatProviderError::Other {
                                message: message.clone(),
                            }) as AgentLlmRequestError
                        })?;
                    }
                }
                ModelRequestEvent::Usage { usage: next, .. } => usage = next,
                ModelRequestEvent::Timing(next) => timing = Some(next),
                ModelRequestEvent::Finish {
                    message,
                    provider_finish_reason,
                    raw_finish_reason,
                    id,
                    trace_id,
                } => {
                    trace.set_trace_id(trace_id.clone());
                    finish = Some((
                        message,
                        provider_finish_reason,
                        raw_finish_reason,
                        id,
                        trace_id,
                    ))
                }
            }
        }
        let Some((
            message,
            provider_finish_reason,
            raw_finish_reason,
            provider_message_id,
            trace_id,
        )) = finish
        else {
            let error = ChatProviderError::Other {
                message: "LLM request stream ended without a finish event.".into(),
            };
            return Err(Arc::new(error));
        };
        Ok(AgentLlmRequestFinish {
            message,
            usage,
            model: Some(requester.model().id.clone()),
            provider_finish_reason,
            raw_finish_reason,
            provider_message_id,
            timing,
            trace_id,
        })
    }
}

#[async_trait]
impl AgentLlmRequesterServiceContract for AgentLlmRequesterService {
    fn prepare_turn_config(&self, turn_id: i64) -> Option<PreparedTurnRequestConfig> {
        if !self.profile.has_provider() {
            return None;
        }
        self.turn_config(turn_id)
            .ok()
            .map(|config| PreparedTurnRequestConfig {
                thinking_effort: config.0.thinking_level,
            })
    }
    async fn request(
        &self,
        overrides: Option<AgentLlmRequestOverrides>,
        on_part: Option<AgentLlmRequestPartHandler>,
        signal: Option<AbortSignal>,
    ) -> Result<AgentLlmRequestFinish, AgentLlmRequestError> {
        self.perform(
            overrides.unwrap_or_default(),
            on_part,
            signal,
            LlmRequestTrace::default(),
        )
        .await
    }
    fn start(
        &self,
        overrides: Option<AgentLlmRequestOverrides>,
        on_part: Option<AgentLlmRequestPartHandler>,
        signal: Option<AbortSignal>,
    ) -> AgentLlmRequestTask {
        let service = Arc::new(self.clone_for_task());
        let trace = LlmRequestTrace::default();
        let result_trace = trace.clone();
        AgentLlmRequestTask {
            trace,
            result: async move {
                service
                    .perform(overrides.unwrap_or_default(), on_part, signal, result_trace)
                    .await
            }
            .boxed(),
        }
    }
}

impl AgentLlmRequesterService {
    fn clone_for_task(&self) -> Self {
        Self {
            context: self.context.clone(),
            projector: self.projector.clone(),
            context_size: self.context_size.clone(),
            tools: self.tools.clone(),
            tool_select: self.tool_select.clone(),
            profile: self.profile.clone(),
            usage: self.usage.clone(),
            catalog: self.catalog.clone(),
            fault: self.fault.clone(),
            turn_configs: self.turn_configs.clone_for_task(),
        }
    }
}

fn find_provider_error<'a>(mut error: &'a (dyn Error + 'static)) -> Option<&'a ChatProviderError> {
    loop {
        if let Some(error) = error.downcast_ref::<ChatProviderError>() {
            return Some(error);
        }
        error = error.source()?;
    }
}

pub fn register_agent_llm_requester_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_LLM_REQUESTER_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let context = accessor.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?;
            let projector = accessor.get(AGENT_CONTEXT_PROJECTOR_SERVICE_ID)?;
            let context_size = accessor.get(AGENT_CONTEXT_SIZE_SERVICE_ID)?;
            let tools = accessor.get(AGENT_TOOL_REGISTRY_SERVICE_ID)?;
            let tool_select = accessor.get(AGENT_TOOL_SELECT_SERVICE_ID)?;
            let profile = accessor.get(AGENT_PROFILE_SERVICE_ID)?;
            let usage = accessor.get(AGENT_USAGE_SERVICE_ID)?;
            let catalog = accessor.get(MODEL_CATALOG_SERVICE_ID)?;
            let fault = accessor.get(FAULT_INJECTION_SERVICE_ID)?;
            Ok(AgentLlmRequesterServiceHandle(Arc::new(
                AgentLlmRequesterService::new(
                    (*context).clone(),
                    (*projector).clone(),
                    (*context_size).clone(),
                    (*tools).clone(),
                    (*tool_select).clone(),
                    (*profile).clone(),
                    (*usage).clone(),
                    (*catalog).clone(),
                    (*fault).clone(),
                ),
            )))
        }),
        InstantiationType::Eager,
        "llmRequester",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::{
        errors::errors::{Error2, Error2Options, ErrorCause},
        utils::abort::{AbortController, AbortError},
    };

    #[tokio::test]
    async fn abort_signal_bridge_cancels_the_provider_token() {
        let controller = AbortController::new();
        let bridge = AbortSignalBridge::new(controller.signal());
        assert!(!bridge.token.is_cancelled());

        controller.abort(Some(AbortError::new("cancel request")));
        tokio::time::timeout(std::time::Duration::from_secs(1), bridge.token.cancelled())
            .await
            .expect("provider cancellation must be propagated");
    }

    #[test]
    fn projection_recovery_can_inspect_translated_provider_causes() {
        let provider: Arc<dyn Error + Send + Sync> =
            Arc::new(ChatProviderError::ApiRequestTooLarge {
                message: "request body too large".into(),
                data: ApiStatusData::new(413, None, None, Some("trace-413".into())),
            });
        let translated = Error2::with_options(
            "provider.api_error",
            "request body too large",
            Error2Options {
                cause: Some(ErrorCause::Error(provider)),
                ..Error2Options::default()
            },
        );

        assert!(matches!(
            find_provider_error(&translated),
            Some(ChatProviderError::ApiRequestTooLarge { .. })
        ));
    }

    #[test]
    fn task_clones_reuse_prepared_turn_config() {
        let service_cache = TurnConfigCache::default();
        let task_cache = service_cache.clone_for_task();
        service_cache.insert(
            7,
            (
                ProfileModelContext {
                    model_alias: "frozen-model".into(),
                    model_capabilities: crate::kosong::contract::capability::UNKNOWN_CAPABILITY
                        .clone(),
                    max_output_size: None,
                    always_thinking: None,
                    thinking_level: crate::kosong::contract::provider::ThinkingEffort::from("high"),
                    reserved_context_size: None,
                    compaction_trigger_ratio: None,
                },
                ModelRequestParams::default(),
                "frozen system prompt".into(),
            ),
        );

        let config = task_cache.get(7).expect("task clone must see turn config");
        assert_eq!(config.0.model_alias, "frozen-model");
        assert_eq!(config.0.thinking_level.as_str(), "high");
        assert_eq!(config.2, "frozen system prompt");
    }
}
