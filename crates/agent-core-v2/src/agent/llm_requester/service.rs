//! Agent LLM requester implementation.
//!
//! Original: `packages/agent-core-v2/src/agent/llmRequester/llmRequesterService.ts`.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_util::{StreamExt, future::FutureExt};

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
        profile::{AGENT_PROFILE_SERVICE_ID, AgentProfileServiceHandle},
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
    AGENT_LLM_REQUESTER_SERVICE_ID, AgentLlmRequestFinish, AgentLlmRequestOverrides,
    AgentLlmRequestPartHandler, AgentLlmRequestSource, AgentLlmRequestTask,
    AgentLlmRequesterServiceContract, AgentLlmRequesterServiceHandle, PreparedTurnRequestConfig,
};

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
    turn_configs: Mutex<
        std::collections::HashMap<
            i64,
            (
                crate::agent::profile::ProfileModelContext,
                ModelRequestParams,
                String,
            ),
        >,
    >,
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
            turn_configs: Mutex::new(Default::default()),
        }
    }

    fn turn_config(
        &self,
        id: i64,
    ) -> Result<
        (
            crate::agent::profile::ProfileModelContext,
            ModelRequestParams,
            String,
        ),
        String,
    > {
        if let Some(config) = self.turn_configs.lock().unwrap().get(&id).cloned() {
            return Ok(config);
        }
        let config = (
            self.profile
                .resolve_model_context()
                .map_err(|e| e.to_string())?,
            self.profile
                .resolve_request_params()
                .map_err(|e| e.to_string())?,
            self.profile.get_system_prompt(),
        );
        let mut configs = self.turn_configs.lock().unwrap();
        configs.retain(|key, _| *key >= id);
        configs.insert(id, config.clone());
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
    ) -> Result<AgentLlmRequestFinish, String> {
        if let Some(signal) = &signal {
            signal.throw_if_aborted().map_err(|e| e.to_string())?;
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
                    .map_err(|e| e.to_string())?,
                self.profile
                    .resolve_request_params()
                    .map_err(|e| e.to_string())?,
                self.profile.get_system_prompt(),
            ),
        };
        let requester = self
            .catalog
            .get_requester(&resolved.model_alias)
            .map_err(|e| e.to_string())?;
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
            let messages = match projection {
                0 => self.projector.project(&shaped),
                1 => self.projector.project_strict(&shaped),
                2 => self.projector.project_media_degraded(&shaped),
                _ => self.projector.project_media_stripped(&shaped, None),
            }
            .map_err(|e| e.to_string())?;
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
                )
                .await
            {
                Ok(finish) => {
                    let _ = self.usage.record(
                        resolved.model_alias,
                        finish.usage.clone(),
                        overrides.source.clone(),
                    );
                    let _ = self.context_size.measured(
                        &messages,
                        std::slice::from_ref(&finish.message),
                        finish.usage.clone(),
                    );
                    return Ok(finish);
                }
                Err((error, message)) => {
                    if signal.as_ref().is_some_and(AbortSignal::aborted) {
                        return Err(message);
                    }
                    projection = match (&error, projection) { (ChatProviderError::ApiRequestTooLarge { .. }, 0) => 2, (ChatProviderError::ApiRequestTooLarge { .. }, 2) => 3, (_, 0) if crate::kosong::contract::errors::is_image_format_error(&error) => 3, (_, 0) if crate::kosong::contract::errors::is_recoverable_request_structure_error(&error) => 1, _ => return Err(message) };
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
        _signal: Option<AbortSignal>,
    ) -> Result<AgentLlmRequestFinish, (ChatProviderError, String)> {
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
            return Err((error.clone(), error.to_string()));
        }
        let mut stream = requester.request(input, None, Some(params));
        let mut usage = empty_usage();
        let mut timing = None;
        let mut finish = None;
        while let Some(event) = stream.next().await {
            match event.map_err(|error| match error {
                crate::kosong::model::ModelRequestError::Abort(error) => {
                    (error.clone(), error.to_string())
                }
                crate::kosong::model::ModelRequestError::Coded(error) => {
                    let provider = ChatProviderError::Other {
                        message: error.to_string(),
                    };
                    (provider.clone(), provider.to_string())
                }
            })? {
                ModelRequestEvent::Part(part) => {
                    if let Some(handler) = &on_part {
                        handler(part).await.map_err(|message| {
                            let error = ChatProviderError::Other {
                                message: message.clone(),
                            };
                            (error, message)
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
            return Err((error.clone(), error.to_string()));
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
    ) -> Result<AgentLlmRequestFinish, String> {
        self.perform(overrides.unwrap_or_default(), on_part, signal)
            .await
    }
    fn start(
        &self,
        overrides: Option<AgentLlmRequestOverrides>,
        on_part: Option<AgentLlmRequestPartHandler>,
        signal: Option<AbortSignal>,
    ) -> AgentLlmRequestTask {
        let service = Arc::new(self.clone_for_task());
        AgentLlmRequestTask {
            trace: LlmRequestTrace::default(),
            result: async move {
                service
                    .perform(overrides.unwrap_or_default(), on_part, signal)
                    .await
            }
            .boxed(),
        }
    }
}

impl AgentLlmRequesterService {
    fn clone_for_task(&self) -> Self {
        Self::new(
            self.context.clone(),
            self.projector.clone(),
            self.context_size.clone(),
            self.tools.clone(),
            self.tool_select.clone(),
            self.profile.clone(),
            self.usage.clone(),
            self.catalog.clone(),
            self.fault.clone(),
        )
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
