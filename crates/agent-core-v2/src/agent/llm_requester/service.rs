//! Agent LLM requester implementation.
//!
//! Original: `packages/agent-core-v2/src/agent/llmRequester/llmRequesterService.ts`.

use parking_lot::Mutex;
use std::sync::{Arc, LazyLock};
use std::{
    collections::{HashMap, HashSet},
    error::Error,
};

use async_trait::async_trait;
use futures_util::{StreamExt, future::FutureExt};
use serde_json::{Map, Value};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        utils::{abort::AbortSignal, hash::sha256_hex},
    },
    agent::{
        context_memory::{AGENT_CONTEXT_MEMORY_SERVICE_ID, AgentContextMemoryServiceHandle},
        context_projector::{
            AGENT_CONTEXT_PROJECTOR_SERVICE_ID, AgentContextProjectorServiceHandle,
            MediaStripSnapshot,
        },
        context_size::{AGENT_CONTEXT_SIZE_SERVICE_ID, AgentContextSizeServiceHandle},
        fault_injection::{FAULT_INJECTION_SERVICE_ID, FaultInjectionServiceHandle, FaultKind},
        profile::{AGENT_PROFILE_SERVICE_ID, AgentProfileServiceHandle, ProfileModelContext},
        tool_registry::{AGENT_TOOL_REGISTRY_SERVICE_ID, AgentToolRegistryServiceHandle},
        tool_select::{AGENT_TOOL_SELECT_SERVICE_ID, AgentToolSelectServiceHandle},
        usage::{AGENT_USAGE_SERVICE_ID, AgentUsageServiceHandle},
    },
    app::config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
    kosong::{
        contract::{
            errors::{ApiStatusData, ChatProviderError},
            message::Message,
            provider::ThinkingEffort,
            request_trace::LlmRequestTrace,
            tool::Tool,
            usage::empty_usage,
        },
        model::{
            MODEL_CATALOG_SERVICE_ID, Model, ModelCatalogHandle, ModelRequestEvent,
            ModelRequestInput, ModelRequestParams,
            completion_budget::{
                ResolveCompletionBudgetArgs, completion_budget_params, resolve_completion_budget,
            },
            contract::{MODELS_SECTION, ModelsSection},
            effective_max_completion_tokens,
            thinking::{
                THINKING_SECTION, ThinkingConfig, default_thinking_effort_for_model,
                resolve_thinking_keep,
            },
            types::{ModelOverrides, ModelThinkingCapabilities, ModelThinkingMetadata},
        },
    },
    wire::contract::{WIRE_SERVICE_ID, WireServiceHandle},
};

use super::{
    AGENT_LLM_REQUESTER_SERVICE_ID, AgentLlmRequestError, AgentLlmRequestFinish,
    AgentLlmRequestLogFields, AgentLlmRequestOverrides, AgentLlmRequestPartHandler,
    AgentLlmRequestSource, AgentLlmRequestTask, AgentLlmRequesterServiceContract,
    AgentLlmRequesterServiceHandle, LLM_REQUEST, LLM_REQUEST_TRACE_MODEL, LLM_TOOLS_SNAPSHOT,
    LlmRequestKind, LlmRequestPayload, LlmRequestProjection, LlmRequestToolSchema,
    LlmToolsSnapshotPayload, PreparedTurnRequestConfig, llm_request, llm_tools_snapshot,
};

type TurnRequestConfig = (ProfileModelContext, ModelRequestParams, String);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestProjection {
    Normal,
    Strict,
    MediaDegraded,
    MediaStripped,
}

impl RequestProjection {
    fn as_field(self) -> Option<&'static str> {
        match self {
            Self::Normal => None,
            Self::Strict => Some("strict"),
            Self::MediaDegraded => Some("media-degraded"),
            Self::MediaStripped => Some("media-stripped"),
        }
    }
}

struct LlmRequestRecordInput<'a> {
    model: &'a Model,
    model_alias: &'a str,
    thinking_effort: &'a ThinkingEffort,
    params: &'a ModelRequestParams,
    system_prompt: &'a str,
    tools: &'a [Tool],
    messages: &'a [Message],
    fields: &'a AgentLlmRequestLogFields,
}

#[derive(Default)]
struct TurnConfigCache {
    configs: Arc<Mutex<HashMap<crate::agent::TurnId, TurnRequestConfig>>>,
}

impl TurnConfigCache {
    fn clone_for_task(&self) -> Self {
        Self {
            configs: Arc::clone(&self.configs),
        }
    }

    fn get(&self, id: crate::agent::TurnId) -> Option<TurnRequestConfig> {
        self.configs.lock().get(&id).cloned()
    }

    fn insert(&self, id: crate::agent::TurnId, config: TurnRequestConfig) {
        let mut configs = self.configs.lock();
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
    config: ConfigServiceHandle,
    wire: WireServiceHandle,
    turn_configs: TurnConfigCache,
    media_degraded_turns: Arc<Mutex<HashSet<crate::agent::TurnId>>>,
    media_stripped_turns: Arc<Mutex<HashMap<crate::agent::TurnId, MediaStripSnapshot>>>,
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
        config: ConfigServiceHandle,
        wire: WireServiceHandle,
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
            config,
            wire,
            turn_configs: TurnConfigCache::default(),
            media_degraded_turns: Arc::new(Mutex::new(HashSet::new())),
            media_stripped_turns: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn turn_config(
        &self,
        id: crate::agent::TurnId,
    ) -> Result<TurnRequestConfig, AgentLlmRequestError> {
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

    fn model_overrides(&self) -> ModelOverrides {
        self.config
            .get("modelOverrides")
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    fn thinking_config(&self) -> ThinkingConfig {
        self.config
            .get(THINKING_SECTION)
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    fn model_beta_api(&self, model_alias: &str) -> Option<bool> {
        self.config
            .get(MODELS_SECTION)
            .and_then(|value| serde_json::from_value::<ModelsSection>(value).ok())
            .and_then(|models| models.get(model_alias).and_then(|model| model.beta_api))
    }

    fn media_strip_snapshot_for_turn(
        &self,
        source: Option<&AgentLlmRequestSource>,
    ) -> Option<MediaStripSnapshot> {
        let turn_id = source_turn_id(source)?;
        self.media_stripped_turns.lock().get(&turn_id).cloned()
    }

    fn is_media_degraded_recovery_turn(&self, source: Option<&AgentLlmRequestSource>) -> bool {
        source_turn_id(source)
            .is_some_and(|turn_id| self.media_degraded_turns.lock().contains(&turn_id))
    }

    fn mark_media_degraded_recovery_turn(&self, source: Option<&AgentLlmRequestSource>) {
        let Some(turn_id) = source_turn_id(source) else {
            return;
        };
        let mut turns = self.media_degraded_turns.lock();
        turns.retain(|candidate| *candidate >= turn_id);
        turns.insert(turn_id);
    }

    fn mark_media_stripped_recovery_turn(
        &self,
        snapshot: MediaStripSnapshot,
        source: Option<&AgentLlmRequestSource>,
    ) {
        let Some(turn_id) = source_turn_id(source) else {
            return;
        };
        let mut turns = self.media_stripped_turns.lock();
        turns.retain(|candidate, _| *candidate >= turn_id);
        turns.insert(turn_id, snapshot);
    }

    fn record_request(&self, input: LlmRequestRecordInput<'_>) -> Result<(), AgentLlmRequestError> {
        let tools = tool_signature(provider_visible_tools(input.tools));
        let tools_json = serde_json::to_string(&tools)
            .map_err(|error| Arc::new(error) as AgentLlmRequestError)?;
        let tools_hash = fingerprint(&tools_json);

        let model_overrides = self.model_overrides();
        let thinking_config = self.thinking_config();
        let system_prompt_hash = fingerprint(input.system_prompt);
        let system_prompt = (input.system_prompt != self.profile.get_system_prompt())
            .then(|| input.system_prompt.to_owned());
        let payload = LlmRequestPayload {
            kind: request_kind_for_record(input.fields),
            provider: input.model.protocol.to_string(),
            model: input.model.name.clone(),
            model_alias: Some(input.model_alias.to_owned()),
            thinking_effort: Some(input.thinking_effort.clone()),
            thinking_keep: resolve_thinking_keep(
                model_overrides.thinking_keep.as_deref(),
                thinking_config.keep.as_deref(),
                input.thinking_effort,
            ),
            temperature: model_overrides.temperature,
            top_p: model_overrides.top_p,
            max_tokens: effective_max_completion_tokens(Some(input.params)),
            beta_api: self.model_beta_api(input.model_alias),
            tool_select: self.tool_select.enabled(),
            system_prompt_hash,
            system_prompt,
            tools_hash: tools_hash.clone(),
            message_count: input.messages.len() as u64,
            turn_step: string_field(input.fields, "turnStep"),
            attempt: string_field(input.fields, "attempt"),
            projection: projection_field(input.fields),
            dropped_count: number_field(input.fields, "droppedCount").map(|value| value as u64),
        };
        dispatch_request_trace(&self.wire, tools_hash, tools, payload)
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
        let source_turn = source_turn_id(overrides.source.as_ref());
        let (mut resolved, mut params, prompt) = match source_turn {
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
        if let Some(model_alias) = overrides.model_alias.as_deref() {
            let model = self
                .catalog
                .get(model_alias)
                .map_err(AgentLlmRequestError::from)?;
            resolved.model_alias = model_alias.to_owned();
            resolved.model_capabilities = model.capabilities.clone();
            resolved.max_output_size = model.max_output_size;
            resolved.always_thinking = model.always_thinking.then_some(true);
            resolved.thinking_level =
                default_thinking_effort_for_model(Some(&ModelThinkingMetadata {
                    capabilities: Some(ModelThinkingCapabilities::Structured(
                        model.capabilities.clone(),
                    )),
                    adaptive_thinking: None,
                    always_thinking: Some(model.always_thinking),
                    support_efforts: model.support_efforts.clone(),
                    default_effort: model.default_effort.clone(),
                }));
            params.thinking_effort = Some(resolved.thinking_level.clone());
            params.thinking_keep = None;
        }
        let model_overrides = self.model_overrides();
        let budget = resolve_completion_budget(ResolveCompletionBudgetArgs {
            max_output_size: overrides.max_output_size.or(resolved.max_output_size),
            reserved_context_size: resolved.reserved_context_size,
            max_completion_tokens_cap: model_overrides.max_completion_tokens,
        });
        let used_context_tokens = overrides
            .messages
            .is_none()
            .then(|| self.context_size.get(None, None).measured);
        if let Some(budget_params) = completion_budget_params(
            budget,
            Some(&resolved.model_capabilities),
            used_context_tokens,
        ) {
            params.max_completion_tokens = Some(budget_params.max_completion_tokens);
            params.used_context_tokens = budget_params.used_context_tokens;
            params.max_context_tokens = budget_params.max_context_tokens;
        }
        let requester = self
            .catalog
            .get_requester(&resolved.model_alias)
            .map_err(AgentLlmRequestError::from)?;
        let model = requester.model();
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
                        attachments: Vec::new(),
                    })
                    .collect()
            })
            .unwrap_or_else(|| self.context.get());
        let shaped = self.tool_select.shape_history(&history);
        let tools = overrides.tools.unwrap_or_else(|| self.default_tools());
        let system_prompt = overrides.system_prompt.unwrap_or(prompt);
        let base_fields = log_fields_for_source(overrides.source.as_ref());
        let mut media_strip_snapshot =
            self.media_strip_snapshot_for_turn(overrides.source.as_ref());
        let mut projection = if media_strip_snapshot.is_some() {
            RequestProjection::MediaStripped
        } else if self.is_media_degraded_recovery_turn(overrides.source.as_ref()) {
            RequestProjection::MediaDegraded
        } else {
            RequestProjection::Normal
        };
        loop {
            trace.set_trace_id(None);
            let messages = match projection {
                RequestProjection::Normal => self.projector.project(&shaped),
                RequestProjection::Strict => self.projector.project_strict(&shaped),
                RequestProjection::MediaDegraded => self.projector.project_media_degraded(&shaped),
                RequestProjection::MediaStripped => self
                    .projector
                    .project_media_stripped(&shaped, media_strip_snapshot.as_ref()),
            }
            .map_err(|error| Arc::new(error) as AgentLlmRequestError)?;
            let input = ModelRequestInput {
                system_prompt: system_prompt.clone(),
                tools: tools.clone(),
                messages,
                response_format: None,
            };
            let fields = request_fields(&base_fields, projection);
            self.record_request(LlmRequestRecordInput {
                model: model.as_ref(),
                model_alias: &resolved.model_alias,
                thinking_effort: &resolved.thinking_level,
                params: &params,
                system_prompt: &input.system_prompt,
                tools: &input.tools,
                messages: &input.messages,
                fields: &fields,
            })?;
            match self
                .run_stream(
                    requester.clone(),
                    input,
                    params.clone(),
                    on_part.clone(),
                    signal.clone(),
                    trace.clone(),
                )
                .await
            {
                Ok(mut finish) => {
                    finish.model = Some(resolved.model_alias.clone());
                    let _ = self.usage.record(
                        resolved.model_alias.clone(),
                        finish.usage,
                        overrides.source.clone(),
                    );
                    let _ = self.context_size.measured(
                        &history,
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
                    projection = match (provider_error, projection) {
                        (
                            ChatProviderError::ApiRequestTooLarge { .. },
                            RequestProjection::Normal,
                        ) => {
                            self.mark_media_degraded_recovery_turn(overrides.source.as_ref());
                            RequestProjection::MediaDegraded
                        }
                        (
                            ChatProviderError::ApiRequestTooLarge { .. },
                            RequestProjection::MediaDegraded,
                        ) => {
                            let snapshot = self
                                .projector
                                .capture_media_strip_snapshot(&shaped)
                                .map_err(|error| Arc::new(error) as AgentLlmRequestError)?;
                            self.mark_media_stripped_recovery_turn(
                                snapshot.clone(),
                                overrides.source.as_ref(),
                            );
                            media_strip_snapshot = Some(snapshot);
                            RequestProjection::MediaStripped
                        }
                        (_, current)
                            if current != RequestProjection::MediaStripped
                                && crate::kosong::contract::errors::is_image_format_error(
                                    provider_error,
                                ) =>
                        {
                            let snapshot = self
                                .projector
                                .capture_media_strip_snapshot(&shaped)
                                .map_err(|error| Arc::new(error) as AgentLlmRequestError)?;
                            self.mark_media_stripped_recovery_turn(
                                snapshot.clone(),
                                overrides.source.as_ref(),
                            );
                            media_strip_snapshot = Some(snapshot);
                            RequestProjection::MediaStripped
                        }
                        (_, RequestProjection::Normal)
                            if crate::kosong::contract::errors::is_recoverable_request_structure_error(
                                provider_error,
                            ) =>
                        {
                            RequestProjection::Strict
                        }
                        _ => return Err(error),
                    };
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
    fn prepare_turn_config(
        &self,
        turn_id: crate::agent::TurnId,
    ) -> Option<PreparedTurnRequestConfig> {
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
            config: self.config.clone(),
            wire: self.wire.clone(),
            turn_configs: self.turn_configs.clone_for_task(),
            media_degraded_turns: Arc::clone(&self.media_degraded_turns),
            media_stripped_turns: Arc::clone(&self.media_stripped_turns),
        }
    }
}

fn source_turn_id(source: Option<&AgentLlmRequestSource>) -> Option<crate::agent::TurnId> {
    match source {
        Some(AgentLlmRequestSource::Turn { turn_id, .. }) => Some(*turn_id),
        _ => None,
    }
}

fn log_fields_for_source(source: Option<&AgentLlmRequestSource>) -> AgentLlmRequestLogFields {
    match source {
        Some(AgentLlmRequestSource::Turn {
            turn_id,
            step,
            log_fields,
        }) => {
            let mut fields = log_fields.clone().unwrap_or_default();
            if let Some(step) = step {
                fields.insert(
                    "turnStep".into(),
                    Value::String(format!("{turn_id}.{step}")),
                );
            }
            fields
        }
        Some(AgentLlmRequestSource::Operation {
            request_kind,
            log_fields,
            ..
        }) => {
            let mut fields = log_fields.clone().unwrap_or_default();
            if let Some(request_kind) = request_kind {
                fields.insert("requestKind".into(), Value::String(request_kind.clone()));
            }
            fields
        }
        None => Map::new(),
    }
}

fn request_fields(
    base: &AgentLlmRequestLogFields,
    projection: RequestProjection,
) -> AgentLlmRequestLogFields {
    let mut fields = base.clone();
    if let Some(projection) = projection.as_field() {
        fields.insert("projection".into(), Value::String(projection.into()));
    }
    fields
}

fn provider_visible_tools(tools: &[Tool]) -> impl Iterator<Item = &Tool> {
    tools.iter().filter(|tool| tool.deferred != Some(true))
}

fn tool_signature<'a>(tools: impl IntoIterator<Item = &'a Tool>) -> Vec<LlmRequestToolSchema> {
    tools
        .into_iter()
        .map(|tool| LlmRequestToolSchema {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.parameters.clone(),
        })
        .collect()
}

fn request_kind_for_record(fields: &AgentLlmRequestLogFields) -> LlmRequestKind {
    if fields.get("kind").and_then(Value::as_str) == Some("compaction")
        || fields.get("requestKind").and_then(Value::as_str) == Some("full_compaction")
    {
        LlmRequestKind::Compaction
    } else {
        LlmRequestKind::Loop
    }
}

fn string_field(fields: &AgentLlmRequestLogFields, key: &str) -> Option<String> {
    fields.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn number_field(fields: &AgentLlmRequestLogFields, key: &str) -> Option<f64> {
    fields.get(key).and_then(Value::as_f64)
}

fn projection_field(fields: &AgentLlmRequestLogFields) -> Option<LlmRequestProjection> {
    match fields.get("projection").and_then(Value::as_str) {
        Some("strict") => Some(LlmRequestProjection::Strict),
        Some("media-degraded") => Some(LlmRequestProjection::MediaDegraded),
        Some("media-stripped") => Some(LlmRequestProjection::MediaStripped),
        _ => None,
    }
}

fn fingerprint(content: &str) -> String {
    sha256_hex(content.as_bytes())
}

fn dispatch_request_trace(
    wire: &WireServiceHandle,
    tools_hash: String,
    tools: Vec<LlmRequestToolSchema>,
    payload: LlmRequestPayload,
) -> Result<(), AgentLlmRequestError> {
    if !wire
        .get_model(&LLM_REQUEST_TRACE_MODEL)
        .seen_tools_hashes
        .contains(&tools_hash)
    {
        let op = llm_tools_snapshot(LlmToolsSnapshotPayload {
            hash: tools_hash,
            tools,
        })
        .map_err(|error| Arc::new(error) as AgentLlmRequestError)?;
        wire.dispatch([op])
            .map_err(|error| Arc::new(error) as AgentLlmRequestError)?;
    }

    let op = llm_request(payload).map_err(|error| Arc::new(error) as AgentLlmRequestError)?;
    wire.dispatch([op])
        .map_err(|error| Arc::new(error) as AgentLlmRequestError)
}

fn find_provider_error<'a>(mut error: &'a (dyn Error + 'static)) -> Option<&'a ChatProviderError> {
    loop {
        if let Some(error) = error.downcast_ref::<ChatProviderError>() {
            return Some(error);
        }
        error = error.source()?;
    }
}

fn register_llm_request_ops() {
    // TypeScript registers these descriptors as a module-import side effect.
    // Rust's LazyLock definitions need an explicit force before any agent wire
    // can restore records produced by either implementation.
    LazyLock::force(&LLM_TOOLS_SNAPSHOT);
    LazyLock::force(&LLM_REQUEST);
}

pub fn register_agent_llm_requester_service() {
    register_llm_request_ops();

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
            let config = accessor.get(CONFIG_SERVICE_ID)?;
            let wire = accessor.get(WIRE_SERVICE_ID)?;
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
                    (*config).clone(),
                    (*wire).clone(),
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
    use futures_util::stream;

    use crate::{
        _base::{
            di::lifecycle::{DisposableHandle, disposable_none},
            errors::errors::{Error2, Error2Options, ErrorCause},
            utils::abort::{AbortController, AbortError},
        },
        persistence::interface::append_log_store::{
            AppendLogError, AppendLogOptions, AppendLogStoreHandle, AppendLogStoreService,
            AppendLogValueStream,
        },
        wire::wire_service::{DomainEventPublisher, WireBlobService, WireService},
    };

    #[derive(Default)]
    struct MemoryLog(Mutex<Vec<Value>>);

    #[async_trait]
    impl AppendLogStoreService for MemoryLog {
        fn append_value(&self, _: &str, _: &str, value: Value, _: AppendLogOptions) {
            self.0.lock().push(value);
        }

        fn read_values(&self, _: &str, _: &str) -> AppendLogValueStream {
            Box::pin(stream::iter(self.0.lock().clone().into_iter().map(Ok)))
        }

        async fn rewrite_values(
            &self,
            _: &str,
            _: &str,
            records: Vec<Value>,
        ) -> Result<(), AppendLogError> {
            *self.0.lock() = records;
            Ok(())
        }

        async fn flush(&self) -> Result<(), AppendLogError> {
            Ok(())
        }

        async fn close(&self) -> Result<(), AppendLogError> {
            Ok(())
        }

        fn acquire(&self, _: &str, _: &str) -> DisposableHandle {
            disposable_none()
        }
    }

    struct IdentityBlobs;

    #[async_trait]
    impl WireBlobService for IdentityBlobs {
        async fn offload_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
            Ok(parts)
        }

        async fn load_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
            Ok(parts)
        }
    }

    struct IgnoreEvents;

    impl DomainEventPublisher for IgnoreEvents {
        fn publish(&self, _: Value) {}
    }

    fn trace_wire(log: Arc<MemoryLog>) -> WireServiceHandle {
        WireServiceHandle(Arc::new(WireService::new(
            "agents/llm-requester-test",
            AppendLogStoreHandle(log),
            Arc::new(IdentityBlobs),
            Arc::new(IgnoreEvents),
        )))
    }

    fn request_payload(tools_hash: &str) -> LlmRequestPayload {
        LlmRequestPayload {
            kind: LlmRequestKind::Loop,
            provider: "openai".into(),
            model: "kimi-test".into(),
            model_alias: Some("test".into()),
            thinking_effort: Some(ThinkingEffort::from("high")),
            thinking_keep: Some("all".into()),
            temperature: Some(0.6),
            top_p: Some(0.9),
            max_tokens: Some(4096),
            beta_api: None,
            tool_select: true,
            system_prompt_hash: fingerprint("system"),
            system_prompt: None,
            tools_hash: tools_hash.into(),
            message_count: 2,
            turn_step: Some("1.1".into()),
            attempt: None,
            projection: None,
            dropped_count: None,
        }
    }

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
            crate::agent::TurnId::new(7),
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

        let config = task_cache
            .get(crate::agent::TurnId::new(7))
            .expect("task clone must see turn config");
        assert_eq!(config.0.model_alias, "frozen-model");
        assert_eq!(config.0.thinking_level.as_str(), "high");
        assert_eq!(config.2, "frozen system prompt");
    }

    #[test]
    fn request_fields_match_typescript_source_mapping() {
        let turn = AgentLlmRequestSource::Turn {
            turn_id: crate::agent::TurnId::new(7),
            step: Some(crate::agent::StepId::new(2)),
            log_fields: Some(Map::from_iter([(
                "attempt".into(),
                Value::String("retry-1".into()),
            )])),
        };
        let normal = log_fields_for_source(Some(&turn));
        assert_eq!(normal["turnStep"], "7.2");
        assert_eq!(normal["attempt"], "retry-1");
        assert!(projection_field(&normal).is_none());

        let degraded = request_fields(&normal, RequestProjection::MediaDegraded);
        assert_eq!(
            projection_field(&degraded),
            Some(LlmRequestProjection::MediaDegraded)
        );
        assert_eq!(string_field(&degraded, "turnStep").as_deref(), Some("7.2"));

        let compaction = AgentLlmRequestSource::Operation {
            turn_id: Some(crate::agent::TurnId::new(7)),
            request_kind: Some("full_compaction".into()),
            log_fields: Some(Map::from_iter([("droppedCount".into(), Value::from(3))])),
        };
        let compaction_fields = log_fields_for_source(Some(&compaction));
        assert_eq!(
            request_kind_for_record(&compaction_fields),
            LlmRequestKind::Compaction
        );
        assert_eq!(number_field(&compaction_fields, "droppedCount"), Some(3.0));
    }

    #[test]
    fn tool_snapshot_hash_matches_typescript_json_stringify() {
        let visible = Tool {
            name: "read".into(),
            description: "Read".into(),
            parameters: Map::from_iter([("type".into(), Value::String("object".into()))]),
            deferred: None,
        };
        let deferred = Tool {
            name: "select_tools".into(),
            description: "Deferred".into(),
            parameters: Map::new(),
            deferred: Some(true),
        };

        let signature = tool_signature(provider_visible_tools(&[visible, deferred]));
        let json = serde_json::to_string(&signature).unwrap();
        assert_eq!(
            json,
            r#"[{"name":"read","description":"Read","parameters":{"type":"object"}}]"#
        );
        assert_eq!(
            fingerprint(&json),
            "86c4061e864cf228c524edb67180ab8f17337360d959739f8ebe9fd65a34ae12"
        );
    }

    #[tokio::test]
    async fn request_trace_dispatch_deduplicates_snapshots_across_restore() {
        register_llm_request_ops();
        let log = Arc::new(MemoryLog::default());
        let tools = vec![LlmRequestToolSchema {
            name: "read".into(),
            description: "Read".into(),
            parameters: Map::from_iter([("type".into(), Value::String("object".into()))]),
        }];
        let tools_hash = fingerprint(&serde_json::to_string(&tools).unwrap());
        let first = trace_wire(Arc::clone(&log));

        dispatch_request_trace(
            &first,
            tools_hash.clone(),
            tools.clone(),
            request_payload(&tools_hash),
        )
        .unwrap();
        dispatch_request_trace(
            &first,
            tools_hash.clone(),
            tools.clone(),
            request_payload(&tools_hash),
        )
        .unwrap();
        first.flush().await.unwrap();

        let records = log.0.lock().clone();
        assert_eq!(
            records
                .iter()
                .filter(|record| record["type"] == "llm.tools_snapshot")
                .count(),
            1
        );
        assert_eq!(
            records
                .iter()
                .filter(|record| record["type"] == "llm.request")
                .count(),
            2
        );

        let restored = trace_wire(Arc::clone(&log));
        restored.restore().await.unwrap();
        assert_eq!(
            restored
                .get_model(&LLM_REQUEST_TRACE_MODEL)
                .seen_tools_hashes,
            vec![tools_hash.clone()]
        );
        dispatch_request_trace(
            &restored,
            tools_hash.clone(),
            tools,
            request_payload(&tools_hash),
        )
        .unwrap();
        restored.flush().await.unwrap();

        let records = log.0.lock();
        assert_eq!(
            records
                .iter()
                .filter(|record| record["type"] == "llm.tools_snapshot")
                .count(),
            1
        );
        assert_eq!(
            records
                .iter()
                .filter(|record| record["type"] == "llm.request")
                .count(),
            3
        );
    }

    #[test]
    fn projection_field_accepts_only_wire_schema_values() {
        for (raw, expected) in [
            ("strict", Some(LlmRequestProjection::Strict)),
            ("media-degraded", Some(LlmRequestProjection::MediaDegraded)),
            ("media-stripped", Some(LlmRequestProjection::MediaStripped)),
            ("normal", None),
            ("unknown", None),
        ] {
            let fields = Map::from_iter([("projection".into(), Value::String(raw.into()))]);
            assert_eq!(projection_field(&fields), expected);
        }
    }

    #[test]
    fn runtime_registration_installs_request_trace_ops_before_restore() {
        register_llm_request_ops();
        assert!(
            crate::wire::op::registered_op("llm.tools_snapshot").is_some(),
            "tool snapshots must be replayable before requester instantiation"
        );
        assert!(
            crate::wire::op::registered_op("llm.request").is_some(),
            "request records must be replayable before requester instantiation"
        );
    }
}
