//! Model-requester implementation helpers.
//!
//! Original: `packages/agent-core-v2/src/kosong/model/modelRequesterImpl.ts`.

use parking_lot::Mutex;
use std::sync::Arc;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};

use futures_util::{Stream, future::BoxFuture};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    _base::errors::{codes::CORE_INTERNAL, errors::Error2},
    kosong::{
        contract::{
            errors::{ChatProviderError, create_abort_error, is_abort_error},
            generate::{GenerateCallbacks, GenerateResult, generate},
            provider::{
                ChatProvider, GenerateOptions, ProviderError, ProviderRequestAuth,
                StreamDecodeStats, VideoUploadSource,
            },
        },
        protocol::{
            errors::{ProviderBoundaryError, translate_provider_error},
            identity::{ProtocolAdapterConfig, ProtocolAdapterRegistry},
        },
    },
};

use super::{
    catalog::{AuthRequestOptions, Model},
    model_requester::{
        ModelRequestError, ModelRequestEvent, ModelRequestInput, ModelRequestParams,
        ModelRequestStream, ModelRequestTiming, ModelRequester, UploadVideoFuture,
    },
};

pub struct ModelRequesterImpl {
    model: Arc<Model>,
    protocol_registry: Arc<dyn ProtocolAdapterRegistry>,
    cached_chat_provider: Arc<Mutex<Option<Arc<dyn ChatProvider>>>>,
}

impl ModelRequesterImpl {
    // Original: ModelRequesterImpl.constructor().
    pub fn new(model: Arc<Model>, protocol_registry: Arc<dyn ProtocolAdapterRegistry>) -> Self {
        Self {
            model,
            protocol_registry,
            cached_chat_provider: Arc::new(Mutex::new(None)),
        }
    }

    // Original: resolveChatProvider(). One immutable provider is constructed
    // lazily and shared for this model's lifetime.
    fn resolve_chat_provider(&self) -> Result<Arc<dyn ChatProvider>, ProviderError> {
        let mut cached = self.cached_chat_provider.lock();
        if let Some(provider) = cached.as_ref() {
            return Ok(Arc::clone(provider));
        }
        let provider = self
            .protocol_registry
            .create_chat_provider(ProtocolAdapterConfig {
                protocol: self.model.protocol,
                provider_type: self.model.provider_type.as_ref().map(ToString::to_string),
                base_url: self.model.base_url.clone(),
                model_name: self.model.name.clone(),
                api_key: None,
                default_headers: Some(self.model.headers.clone()),
                provider_options: self.model.provider_options.clone(),
            })?;
        *cached = Some(Arc::clone(&provider));
        Ok(provider)
    }

    async fn get_auth(&self, force: bool) -> Result<Option<ProviderRequestAuth>, ProviderError> {
        self.model
            .auth_provider
            .get_auth(force.then_some(AuthRequestOptions { force: true }))
            .await
            .map_err(|error| {
                Box::new(ChatProviderError::Other {
                    message: error.to_string(),
                }) as ProviderError
            })
    }

    async fn run_with_auth_refresh<T>(
        &self,
        run: impl Fn(Option<ProviderRequestAuth>) -> BoxFuture<'static, Result<T, ProviderError>>,
    ) -> Result<T, RequestExecutionError> {
        let auth = self
            .get_auth(false)
            .await
            .map_err(RequestExecutionError::Provider)?;
        match run(auth).await {
            Ok(value) => Ok(value),
            Err(error) if !self.should_force_refresh(&error) => {
                Err(RequestExecutionError::Provider(error))
            }
            Err(_) => {
                let refreshed = self
                    .get_auth(true)
                    .await
                    .map_err(RequestExecutionError::Provider)?;
                match run(refreshed).await {
                    Ok(value) => Ok(value),
                    Err(error) => {
                        if let Some(status) = error.downcast_ref::<ChatProviderError>()
                            && is_unauthorized_status_error(status)
                        {
                            return match translate_provider_error(ProviderBoundaryError::Provider(
                                status.clone(),
                            )) {
                                Ok(error) => Err(RequestExecutionError::Coded(error)),
                                Err(error) => Err(RequestExecutionError::Provider(Box::new(error))),
                            };
                        }
                        Err(RequestExecutionError::Provider(error))
                    }
                }
            }
        }
    }

    async fn run_request(
        self: Arc<Self>,
        input: ModelRequestInput,
        params: Option<ModelRequestParams>,
        signal: CancellationToken,
        events: mpsc::UnboundedSender<Result<ModelRequestEvent, ModelRequestError>>,
    ) {
        let result = self
            .run_request_inner(input, params, signal, events.clone())
            .await
            .map_err(map_execution_error);
        if let Err(error) = result {
            let _ = events.send(Err(error));
        }
    }

    async fn run_request_inner(
        &self,
        input: ModelRequestInput,
        params: Option<ModelRequestParams>,
        signal: CancellationToken,
        events: mpsc::UnboundedSender<Result<ModelRequestEvent, ModelRequestError>>,
    ) -> Result<(), RequestExecutionError> {
        if signal.is_cancelled() {
            return Err(RequestExecutionError::Provider(Box::new(
                create_abort_error(),
            )));
        }
        let provider = self
            .resolve_chat_provider()
            .map_err(RequestExecutionError::Provider)?;
        let timing = Arc::new(Mutex::new(RequestTimingState::new()));
        let callbacks = callbacks_for(Arc::clone(&timing), events.clone(), signal.clone());
        let on_start_timing = Arc::clone(&timing);
        let on_sent_timing = Arc::clone(&timing);
        let on_end_timing = Arc::clone(&timing);
        let options = GenerateOptions {
            signal: Some(signal),
            auth: None,
            response_format: input.response_format.clone(),
            cache_key: params.as_ref().and_then(|params| params.cache_key.clone()),
            sampling: params.as_ref().and_then(|params| params.sampling),
            thinking: params.as_ref().and_then(|params| {
                params.thinking_effort.clone().map(|effort| {
                    crate::kosong::contract::provider::ThinkingRequestOptions {
                        effort,
                        keep: params.thinking_keep.clone(),
                    }
                })
            }),
            max_completion_tokens: params
                .as_ref()
                .and_then(|params| params.max_completion_tokens),
            used_context_tokens: params
                .as_ref()
                .and_then(|params| params.used_context_tokens),
            max_context_tokens: params.as_ref().and_then(|params| params.max_context_tokens),
            on_request_start: Some(Arc::new(move || {
                on_start_timing.lock().request_started_at = Instant::now();
            })),
            on_request_sent: Some(Arc::new(move || {
                on_sent_timing.lock().request_sent_at = Some(Instant::now());
            })),
            on_stream_end: Some(Arc::new(move |stats| {
                let mut state = on_end_timing.lock();
                state.stream_ended_at = Some(Instant::now());
                state.decode_stats = stats;
            })),
            on_trace_id: params.and_then(|params| params.on_trace_id),
        };
        let result = self
            .generate_with_auth_refresh(provider, input, callbacks, options)
            .await?;
        self.send_result_events(result, timing, &events);
        Ok(())
    }

    async fn generate_with_auth_refresh(
        &self,
        provider: Arc<dyn ChatProvider>,
        input: ModelRequestInput,
        callbacks: GenerateCallbacks,
        options: GenerateOptions,
    ) -> Result<GenerateResult, RequestExecutionError> {
        let input = Arc::new(input);
        self.run_with_auth_refresh(move |auth| {
            let provider = Arc::clone(&provider);
            let input = Arc::clone(&input);
            let callbacks = callbacks.clone();
            let mut options = options.clone();
            options.auth = auth;
            Box::pin(async move {
                generate(
                    provider.as_ref(),
                    &input.system_prompt,
                    &input.tools,
                    &input.messages,
                    Some(&callbacks),
                    Some(&options),
                )
                .await
            })
        })
        .await
    }

    fn send_result_events(
        &self,
        result: GenerateResult,
        timing: Arc<Mutex<RequestTimingState>>,
        events: &mpsc::UnboundedSender<Result<ModelRequestEvent, ModelRequestError>>,
    ) {
        if let Some(usage) = result.usage {
            let _ = events.send(Ok(ModelRequestEvent::Usage {
                usage,
                model: Some(self.model.name.clone()),
            }));
        }
        let _ = events.send(Ok(ModelRequestEvent::Finish {
            message: result.message,
            provider_finish_reason: result.finish_reason,
            raw_finish_reason: result.raw_finish_reason,
            id: result.id,
            trace_id: result.trace_id.flatten(),
        }));
        let timing = timing.lock();
        if let Some(first_chunk_at) = timing.first_chunk_at {
            let _ = events.send(Ok(ModelRequestEvent::Timing(build_stream_timing(
                timing.request_started_at,
                timing.request_sent_at,
                first_chunk_at,
                timing.stream_ended_at,
                timing.decode_stats,
            ))));
        }
    }

    fn should_force_refresh(&self, error: &ProviderError) -> bool {
        self.model.auth_provider.can_refresh()
            && error
                .downcast_ref::<ChatProviderError>()
                .is_some_and(is_unauthorized_status_error)
    }
}

impl ModelRequester for ModelRequesterImpl {
    fn model(&self) -> Arc<Model> {
        Arc::clone(&self.model)
    }

    // Original: request(). The spawned task belongs to the returned stream;
    // dropping the stream cancels and aborts it so no request task escapes its
    // consumer's lifetime.
    fn request(
        &self,
        input: ModelRequestInput,
        signal: Option<CancellationToken>,
        params: Option<ModelRequestParams>,
    ) -> ModelRequestStream {
        let signal = signal.unwrap_or_default().child_token();
        let (sender, receiver) = mpsc::unbounded_channel();
        let requester = Arc::new(self.clone_for_request());
        let task_signal = signal.clone();
        let task = tokio::spawn(async move {
            requester
                .run_request(input, params, task_signal, sender)
                .await;
        });
        Box::pin(ManagedRequestStream {
            receiver,
            signal,
            task,
        })
    }

    fn upload_video(
        &self,
        input: VideoUploadSource,
        signal: Option<CancellationToken>,
    ) -> UploadVideoFuture {
        let requester = Arc::new(self.clone_for_request());
        Box::pin(async move {
            let provider = requester.resolve_chat_provider()?;
            let signal = signal.unwrap_or_default().child_token();
            let result = requester
                .run_with_auth_refresh(move |auth| {
                    let provider = Arc::clone(&provider);
                    let signal = signal.clone();
                    let input = input.clone();
                    Box::pin(async move {
                        provider
                            .upload_video(
                                input,
                                Some(&GenerateOptions {
                                    signal: Some(signal),
                                    auth,
                                    ..GenerateOptions::default()
                                }),
                            )
                            .await
                    })
                })
                .await
                .map_err(|error| match error {
                    RequestExecutionError::Provider(error) => error,
                    RequestExecutionError::Coded(error) => Box::new(error) as ProviderError,
                })?;
            let content = result.ok_or_else(|| {
                Box::new(ChatProviderError::Other {
                    message: format!(
                        "Model \"{}\" (protocol={}) does not support video upload",
                        requester.model.id, requester.model.protocol
                    ),
                }) as ProviderError
            })?;
            Ok(Some(content))
        })
    }
}

impl ModelRequesterImpl {
    fn clone_for_request(&self) -> Self {
        Self {
            model: Arc::clone(&self.model),
            protocol_registry: Arc::clone(&self.protocol_registry),
            cached_chat_provider: Arc::clone(&self.cached_chat_provider),
        }
    }
}

enum RequestExecutionError {
    Provider(ProviderError),
    Coded(Error2),
}

fn map_execution_error(error: RequestExecutionError) -> ModelRequestError {
    match error {
        RequestExecutionError::Coded(error) => ModelRequestError::Coded(error),
        RequestExecutionError::Provider(error) => {
            let Some(error) = error.downcast_ref::<ChatProviderError>() else {
                return ModelRequestError::Coded(Error2::new(CORE_INTERNAL, error.to_string()));
            };
            if is_abort_error(error) {
                return ModelRequestError::Abort(create_abort_error());
            }
            match translate_provider_error(ProviderBoundaryError::Provider(error.clone())) {
                Ok(error) => ModelRequestError::Coded(error),
                Err(error) => ModelRequestError::Abort(error),
            }
        }
    }
}

fn is_unauthorized_status_error(error: &ChatProviderError) -> bool {
    error.status_code() == Some(401)
}

#[derive(Clone, Copy)]
struct RequestTimingState {
    request_started_at: Instant,
    request_sent_at: Option<Instant>,
    first_chunk_at: Option<Instant>,
    stream_ended_at: Option<Instant>,
    decode_stats: Option<StreamDecodeStats>,
}

impl RequestTimingState {
    fn new() -> Self {
        Self {
            request_started_at: Instant::now(),
            request_sent_at: None,
            first_chunk_at: None,
            stream_ended_at: None,
            decode_stats: None,
        }
    }
}

fn callbacks_for(
    timing: Arc<Mutex<RequestTimingState>>,
    events: mpsc::UnboundedSender<Result<ModelRequestEvent, ModelRequestError>>,
    signal: CancellationToken,
) -> GenerateCallbacks {
    let on_part_timing = Arc::clone(&timing);
    let on_part_events = events.clone();
    let on_part_signal = signal.clone();
    GenerateCallbacks {
        on_message_part: Some(Arc::new(move |part| {
            let timing = Arc::clone(&on_part_timing);
            let events = on_part_events.clone();
            let signal = on_part_signal.clone();
            Box::pin(async move {
                timing
                    .lock()
                    .first_chunk_at
                    .get_or_insert_with(Instant::now);
                if events.send(Ok(ModelRequestEvent::Part(part))).is_err() {
                    signal.cancel();
                }
            })
        })),
        on_tool_call: None,
    }
}

struct ManagedRequestStream {
    receiver: mpsc::UnboundedReceiver<Result<ModelRequestEvent, ModelRequestError>>,
    signal: CancellationToken,
    task: JoinHandle<()>,
}

impl Stream for ManagedRequestStream {
    type Item = Result<ModelRequestEvent, ModelRequestError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.poll_recv(context)
    }
}

impl Drop for ManagedRequestStream {
    fn drop(&mut self) {
        self.signal.cancel();
        self.task.abort();
    }
}

// Original: buildStreamTiming(). `Instant` replaces the source's wall-clock
// `Date.now()` values; callers capture all timestamps on the same monotonic
// clock so clock adjustments cannot produce externally visible negative spans.
pub fn build_stream_timing(
    request_started_at: Instant,
    request_sent_at: Option<Instant>,
    first_chunk_at: Instant,
    stream_ended_at: Option<Instant>,
    decode_stats: Option<StreamDecodeStats>,
) -> ModelRequestTiming {
    let output_ended_at = stream_ended_at.unwrap_or_else(Instant::now);
    let first_token_latency_ms = first_chunk_at
        .checked_duration_since(request_started_at)
        .map_or(0, |duration| duration.as_millis() as u64);
    let stream_duration_ms = output_ended_at
        .checked_duration_since(first_chunk_at)
        .map_or(0, |duration| duration.as_millis() as u64);
    let (request_build_ms, server_first_token_ms) = request_sent_at.map_or((None, None), |sent| {
        // Original: min(max(requestSentAt, requestStartedAt), firstChunkAt).
        let clamped_sent_at = sent.clamp(request_started_at, first_chunk_at);
        (
            Some(
                clamped_sent_at
                    .duration_since(request_started_at)
                    .as_millis() as u64,
            ),
            Some(first_chunk_at.duration_since(clamped_sent_at).as_millis() as u64),
        )
    });

    ModelRequestTiming {
        first_token_latency_ms,
        stream_duration_ms,
        request_build_ms,
        server_first_token_ms,
        server_decode_ms: decode_stats.map(|stats| stats.server_decode_ms),
        client_consume_ms: decode_stats.map(|stats| stats.client_consume_ms),
    }
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::{
        collections::VecDeque,
        task::{Context, Poll},
        time::Duration,
    };

    use async_trait::async_trait;
    use futures_util::StreamExt;

    use super::*;
    use crate::kosong::{
        contract::{
            capability::{ModelCapability, UNKNOWN_CAPABILITY},
            errors::ApiStatusData,
            message::{ContentPart, Message, Role, StreamedMessagePart},
            provider::{FinishReason, StreamedMessage, TraceId},
            tokens::estimate_tokens_for_message,
            usage::TokenUsage,
        },
        protocol::{
            identity::{ExplainedCapability, Protocol},
            protocol_base::ResolvedAdapterIdentity,
        },
    };

    struct FakeStream {
        parts: VecDeque<Result<StreamedMessagePart, ProviderError>>,
        usage: Option<TokenUsage>,
    }

    impl Stream for FakeStream {
        type Item = Result<StreamedMessagePart, ProviderError>;

        fn poll_next(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            Poll::Ready(self.parts.pop_front())
        }
    }

    impl StreamedMessage for FakeStream {
        fn id(&self) -> Option<&str> {
            Some("request-1")
        }

        fn usage(&self) -> Option<&TokenUsage> {
            self.usage.as_ref()
        }

        fn finish_reason(&self) -> Option<FinishReason> {
            Some(FinishReason::Completed)
        }

        fn raw_finish_reason(&self) -> Option<&str> {
            Some("stop")
        }

        fn trace_id(&self) -> TraceId<'_> {
            TraceId::Present(Some("trace-1"))
        }
    }

    struct FakeProvider {
        attempts: AtomicUsize,
        reject_first_attempt: bool,
        observed_token_estimates: Mutex<Vec<usize>>,
    }

    #[async_trait]
    impl ChatProvider for FakeProvider {
        fn name(&self) -> &str {
            "fake"
        }

        fn model_name(&self) -> &str {
            "fake-model"
        }

        fn thinking_effort(&self) -> Option<&crate::kosong::contract::provider::ThinkingEffort> {
            None
        }

        fn max_completion_tokens(&self) -> Option<u64> {
            None
        }

        async fn generate(
            &self,
            _system_prompt: &str,
            _tools: &[crate::kosong::contract::tool::Tool],
            _history: &[crate::kosong::contract::message::Message],
            options: Option<&GenerateOptions>,
        ) -> Result<Box<dyn StreamedMessage>, ProviderError> {
            if let Some(callback) = options.and_then(|options| options.on_request_sent.as_ref()) {
                callback();
            }
            if let Some(message) = _history.first() {
                self.observed_token_estimates
                    .lock()
                    .push(estimate_tokens_for_message(message));
            }
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            if self.reject_first_attempt && attempt == 0 {
                return Err(Box::new(ChatProviderError::ApiStatus {
                    message: "expired credential".to_owned(),
                    data: ApiStatusData::new(401, None, None, None),
                }));
            }
            Ok(Box::new(FakeStream {
                parts: VecDeque::from([Ok(StreamedMessagePart::Content(ContentPart::Text {
                    text: "hello".to_owned(),
                }))]),
                usage: Some(TokenUsage {
                    input_other: 1,
                    output: 2,
                    input_cache_read: 0,
                    input_cache_creation: 0,
                }),
            }))
        }
    }

    struct FakeRegistry {
        provider: Arc<dyn ChatProvider>,
        create_calls: AtomicUsize,
        configurations: Mutex<Vec<ProtocolAdapterConfig>>,
    }

    impl ProtocolAdapterRegistry for FakeRegistry {
        fn supported_protocols(&self) -> Vec<Protocol> {
            vec![Protocol::OpenAi]
        }

        fn resolve_adapter_identity(
            &self,
            _protocol: Protocol,
            _provider_type: Option<&str>,
        ) -> ResolvedAdapterIdentity {
            unreachable!("requester does not resolve adapter identities")
        }

        fn resolve_provider_base_id(
            &self,
            _protocol: Protocol,
            _provider_type: Option<&str>,
        ) -> crate::kosong::protocol::protocol_base::ProtocolBaseId {
            unreachable!("requester does not resolve provider base ids")
        }

        fn resolve_capability(
            &self,
            _protocol: Protocol,
            _model_name: &str,
            _provider_type: Option<&str>,
        ) -> ModelCapability {
            unreachable!("requester does not resolve capabilities")
        }

        fn explain_capability(
            &self,
            _protocol: Protocol,
            _model_name: &str,
            _provider_type: Option<&str>,
        ) -> ExplainedCapability {
            unreachable!("requester does not explain capabilities")
        }

        fn create_chat_provider(
            &self,
            config: ProtocolAdapterConfig,
        ) -> Result<Arc<dyn ChatProvider>, ProviderError> {
            self.create_calls.fetch_add(1, Ordering::SeqCst);
            self.configurations.lock().push(config);
            Ok(Arc::clone(&self.provider))
        }
    }

    struct RefreshingAuth {
        requests: Mutex<Vec<bool>>,
    }

    #[async_trait]
    impl super::super::catalog::AuthProvider for RefreshingAuth {
        fn can_refresh(&self) -> bool {
            true
        }

        async fn get_auth(
            &self,
            options: Option<AuthRequestOptions>,
        ) -> Result<Option<ProviderRequestAuth>, Box<dyn std::error::Error + Send + Sync>> {
            self.requests
                .lock()
                .push(options.is_some_and(|options| options.force));
            Ok(Some(ProviderRequestAuth {
                api_key: Some("token".to_owned()),
                headers: None,
            }))
        }
    }

    fn test_model(auth_provider: Arc<dyn super::super::catalog::AuthProvider>) -> Arc<Model> {
        Arc::new(Model {
            id: "provider/model".to_owned(),
            name: "model".to_owned(),
            aliases: Vec::new(),
            protocol: Protocol::OpenAi,
            base_url: Some("https://example.test/v1".to_owned()),
            headers: indexmap::indexmap! { "X-Test".to_owned() => "header".to_owned() },
            capabilities: UNKNOWN_CAPABILITY.clone(),
            max_context_size: 128_000,
            max_output_size: None,
            display_name: None,
            reasoning_key: None,
            reasoning_history: None,
            support_efforts: None,
            default_effort: None,
            always_thinking: false,
            provider_type: None,
            provider_name: "provider".to_owned(),
            auth_provider,
            provider_options: None,
        })
    }

    fn request_input() -> ModelRequestInput {
        ModelRequestInput {
            system_prompt: "system".to_owned(),
            tools: Vec::new(),
            messages: Vec::new(),
            response_format: None,
        }
    }

    fn request_input_with_cached_message(token_estimate: usize) -> ModelRequestInput {
        let message = Message::new(
            Role::User,
            vec![ContentPart::Text {
                text: "hello".to_owned(),
            }],
            Vec::new(),
        );
        assert_eq!(
            message.token_estimate_or_init(|| token_estimate),
            token_estimate
        );
        ModelRequestInput {
            system_prompt: "system".to_owned(),
            tools: Vec::new(),
            messages: vec![message],
            response_format: None,
        }
    }

    #[test]
    fn timing_clamps_sent_timestamp_and_decode_metrics_like_the_source() {
        let started = Instant::now();
        let first = started + Duration::from_millis(30);
        let ended = first + Duration::from_millis(40);
        let timing = build_stream_timing(
            started,
            Some(started + Duration::from_millis(90)),
            first,
            Some(ended),
            Some(StreamDecodeStats {
                server_decode_ms: 5,
                client_consume_ms: 7,
            }),
        );
        assert_eq!(timing.first_token_latency_ms, 30);
        assert_eq!(timing.stream_duration_ms, 40);
        assert_eq!(timing.request_build_ms, Some(30));
        assert_eq!(timing.server_first_token_ms, Some(0));
        assert_eq!(timing.server_decode_ms, Some(5));
        assert_eq!(timing.client_consume_ms, Some(7));
    }

    #[test]
    fn timing_clamps_pre_start_send_time_and_keeps_metrics_absent_when_unreported() {
        let started = Instant::now();
        let first = started + Duration::from_millis(20);
        let timing = build_stream_timing(
            started,
            Some(started - Duration::from_millis(1)),
            first,
            Some(first),
            None,
        );
        assert_eq!(timing.request_build_ms, Some(0));
        assert_eq!(timing.server_first_token_ms, Some(20));
        assert_eq!(timing.server_decode_ms, None);
        assert_eq!(timing.client_consume_ms, None);
    }

    #[tokio::test]
    async fn request_lazily_creates_the_provider_and_preserves_event_order() {
        let auth = Arc::new(RefreshingAuth {
            requests: Mutex::new(Vec::new()),
        });
        let provider = Arc::new(FakeProvider {
            attempts: AtomicUsize::new(0),
            reject_first_attempt: false,
            observed_token_estimates: Mutex::new(Vec::new()),
        });
        let registry = Arc::new(FakeRegistry {
            provider,
            create_calls: AtomicUsize::new(0),
            configurations: Mutex::new(Vec::new()),
        });
        let requester = ModelRequesterImpl::new(test_model(auth), registry.clone());

        assert_eq!(registry.create_calls.load(Ordering::SeqCst), 0);
        let events = requester
            .request(request_input(), None, None)
            .collect::<Vec<_>>()
            .await;
        let second_events = requester
            .request(request_input(), None, None)
            .collect::<Vec<_>>()
            .await;

        assert!(events.iter().all(Result::is_ok));
        assert_eq!(events.len(), 4);
        assert!(matches!(
            events[0],
            Ok(ModelRequestEvent::Part(StreamedMessagePart::Content(ContentPart::Text { ref text })))
                if text == "hello"
        ));
        assert!(matches!(
            events[1],
            Ok(ModelRequestEvent::Usage { ref usage, model: Some(ref model) })
                if usage.output == 2 && model == "model"
        ));
        assert!(matches!(events[2], Ok(ModelRequestEvent::Finish { .. })));
        assert!(matches!(events[3], Ok(ModelRequestEvent::Timing(_))));
        assert!(second_events.iter().all(Result::is_ok));
        assert_eq!(registry.create_calls.load(Ordering::SeqCst), 1);
        let configurations = registry.configurations.lock();
        assert_eq!(configurations[0].model_name, "model");
        assert_eq!(
            configurations[0].base_url.as_deref(),
            Some("https://example.test/v1")
        );
        assert_eq!(
            configurations[0]
                .default_headers
                .as_ref()
                .and_then(|headers| headers.get("X-Test")),
            Some(&"header".to_owned())
        );
    }

    #[tokio::test]
    async fn request_retries_one_unauthorized_response_with_forced_auth_refresh() {
        let auth = Arc::new(RefreshingAuth {
            requests: Mutex::new(Vec::new()),
        });
        let provider = Arc::new(FakeProvider {
            attempts: AtomicUsize::new(0),
            reject_first_attempt: true,
            observed_token_estimates: Mutex::new(Vec::new()),
        });
        let registry = Arc::new(FakeRegistry {
            provider: provider.clone(),
            create_calls: AtomicUsize::new(0),
            configurations: Mutex::new(Vec::new()),
        });
        let requester = ModelRequesterImpl::new(test_model(auth.clone()), registry);

        let events = requester
            .request(request_input_with_cached_message(123_456), None, None)
            .collect::<Vec<_>>()
            .await;

        assert!(events.iter().all(Result::is_ok));
        assert_eq!(provider.attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            *provider.observed_token_estimates.lock(),
            vec![123_456, 123_456]
        );
        assert_eq!(*auth.requests.lock(), vec![false, true]);
    }

    #[tokio::test]
    async fn upload_video_reports_a_provider_without_upload_support() {
        let auth = Arc::new(RefreshingAuth {
            requests: Mutex::new(Vec::new()),
        });
        let provider = Arc::new(FakeProvider {
            attempts: AtomicUsize::new(0),
            reject_first_attempt: false,
            observed_token_estimates: Mutex::new(Vec::new()),
        });
        let registry = Arc::new(FakeRegistry {
            provider,
            create_calls: AtomicUsize::new(0),
            configurations: Mutex::new(Vec::new()),
        });
        let requester = ModelRequesterImpl::new(test_model(auth), registry);

        let error = requester
            .upload_video(VideoUploadSource::Location("video.mp4".to_owned()), None)
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Model \"provider/model\" (protocol=openai) does not support video upload"
        );
    }
}
