use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use super::errors::{ChatProviderError, create_abort_error};
use super::message::{
    ContentPart, Message, Role, StreamIndex, StreamedMessagePart, ToolCall, merge_in_place,
};
use super::provider::{
    ChatProvider, FinishReason, GenerateOptions, ProviderError, StreamDecodeStats, StreamedMessage,
    TraceId,
};
use super::tool::Tool;
use super::usage::TokenUsage;

// Original: generate.ts, GenerateResult
#[derive(Debug, Clone, PartialEq)]
pub struct GenerateResult {
    pub id: Option<String>,
    pub message: Message,
    pub usage: Option<TokenUsage>,
    pub finish_reason: Option<FinishReason>,
    pub raw_finish_reason: Option<String>,
    // Outer Option preserves the optional property; inner Option preserves
    // its explicit null value.
    pub trace_id: Option<Option<String>>,
}

pub type MessagePartCallback =
    Arc<dyn Fn(StreamedMessagePart) -> BoxFuture<'static, ()> + Send + Sync>;
pub type ToolCallCallback =
    Arc<dyn for<'a> Fn(&'a mut ToolCall) -> BoxFuture<'a, ()> + Send + Sync>;

#[derive(Clone, Default)]
pub struct GenerateCallbacks {
    pub on_message_part: Option<MessagePartCallback>,
    pub on_tool_call: Option<ToolCallCallback>,
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/generate.ts
//   generate()
pub async fn generate(
    provider: &dyn ChatProvider,
    system_prompt: &str,
    tools: &[Tool],
    history: &[Message],
    callbacks: Option<&GenerateCallbacks>,
    options: Option<&GenerateOptions>,
) -> Result<GenerateResult, ProviderError> {
    let mut message = Message::new(Role::Assistant, Vec::new(), Vec::new());
    let mut pending_part: Option<StreamedMessagePart> = None;
    let mut tool_call_index_map = HashMap::<StreamIndex, usize>::new();

    if is_aborted(options) {
        return Err(Box::new(create_abort_error()));
    }

    let wire_tools = if tools.iter().any(|tool| tool.deferred == Some(true)) {
        Cow::Owned(
            tools
                .iter()
                .filter(|tool| tool.deferred != Some(true))
                .cloned()
                .collect(),
        )
    } else {
        Cow::Borrowed(tools)
    };

    if let Some(callback) = options.and_then(|options| options.on_request_start.as_ref()) {
        callback();
    }
    let mut stream = provider
        .generate(system_prompt, wire_tools.as_ref(), history, options)
        .await?;

    let initial_trace_id = owned_trace_id(stream.trace_id());
    if let Some(trace_id) = initial_trace_id.as_ref()
        && let Some(callback) = options.and_then(|options| options.on_trace_id.as_ref())
    {
        callback(trace_id.as_deref());
    }

    throw_if_aborted(options, Some(stream.as_mut())).await?;

    let mut server_decode_ms = 0.0;
    let mut client_consume_ms = 0.0;
    let mut first_part_at: Option<Instant> = None;
    let mut last_resume_at: Option<Instant> = None;

    while let Some(part) = stream.next().await {
        let part = part?;
        let arrived_at = Instant::now();
        if first_part_at.is_none() {
            first_part_at = Some(arrived_at);
        } else if let Some(last_resume_at) = last_resume_at {
            server_decode_ms += arrived_at.duration_since(last_resume_at).as_secs_f64() * 1_000.0;
        }

        let result = consume_part(
            &mut message,
            &mut pending_part,
            &mut tool_call_index_map,
            part,
            callbacks,
            options,
            stream.as_mut(),
        )
        .await;
        let resumed_at = Instant::now();
        client_consume_ms += resumed_at.duration_since(arrived_at).as_secs_f64() * 1_000.0;
        last_resume_at = Some(resumed_at);
        result?;
    }

    throw_if_aborted(options, Some(stream.as_mut())).await?;
    if first_part_at.is_some()
        && let Some(last_resume_at) = last_resume_at
    {
        server_decode_ms += Instant::now().duration_since(last_resume_at).as_secs_f64() * 1_000.0;
    }
    if let Some(callback) = options.and_then(|options| options.on_stream_end.as_ref()) {
        callback(first_part_at.map(|_| StreamDecodeStats {
            server_decode_ms,
            client_consume_ms,
        }));
    }

    if let Some(part) = pending_part {
        flush_part(&mut message, part, &mut tool_call_index_map);
    }

    if message.content.is_empty() && message.tool_calls.is_empty() {
        return Err(Box::new(empty_response_error(
            "The API returned an empty response (no content, no tool calls).",
            provider,
            stream.as_ref(),
        )));
    }

    let has_think = message
        .content
        .iter()
        .any(|part| matches!(part, ContentPart::Think { .. }));
    let has_text = message
        .content
        .iter()
        .any(|part| matches!(part, ContentPart::Text { text } if !text.trim().is_empty()));
    if has_think && !has_text && message.tool_calls.is_empty() {
        return Err(Box::new(empty_response_error(
            "The API returned a response containing only thinking content without any text or tool calls. This usually indicates the stream was interrupted or the output token budget was exhausted during reasoning.",
            provider,
            stream.as_ref(),
        )));
    }

    if let Some(callback) = callbacks.and_then(|callbacks| callbacks.on_tool_call.as_ref()) {
        for tool_call in &mut message.tool_calls {
            throw_if_aborted(options, Some(stream.as_mut())).await?;
            callback(tool_call).await;
        }
    }

    // Read final metadata only after the stream has been fully consumed: the
    // provider implementations update these fields while decoding events.
    Ok(GenerateResult {
        id: stream.id().map(str::to_owned),
        usage: stream.usage().copied(),
        finish_reason: stream.finish_reason(),
        raw_finish_reason: stream.raw_finish_reason().map(str::to_owned),
        trace_id: owned_trace_id(stream.trace_id()),
        message,
    })
}

async fn consume_part(
    message: &mut Message,
    pending_part: &mut Option<StreamedMessagePart>,
    tool_call_index_map: &mut HashMap<StreamIndex, usize>,
    part: StreamedMessagePart,
    callbacks: Option<&GenerateCallbacks>,
    options: Option<&GenerateOptions>,
    stream: &mut dyn StreamedMessage,
) -> Result<(), ProviderError> {
    throw_if_aborted(options, Some(stream)).await?;

    if let Some(callback) = callbacks.and_then(|callbacks| callbacks.on_message_part.as_ref()) {
        callback(part.clone()).await;
        throw_if_aborted(options, Some(stream)).await?;
    }

    if let StreamedMessagePart::ToolCallPart(delta) = &part
        && let Some(index) = delta.index.as_ref()
        && !is_pending_tool_call_at_index(pending_part.as_ref(), index)
        && let Some(array_index) = tool_call_index_map.get(index).copied()
    {
        if let Some(target) = message.tool_calls.get_mut(array_index)
            && let Some(arguments_part) = delta.arguments_part.as_ref()
        {
            if let Some(arguments) = target.arguments.as_mut() {
                arguments.push_str(arguments_part);
            } else {
                target.arguments = Some(arguments_part.clone());
            }
        }
        return Ok(());
    }

    if pending_part.is_none() {
        *pending_part = Some(part);
    } else if !merge_in_place(pending_part.as_mut().unwrap(), &part) {
        let pending = pending_part.take().unwrap();
        flush_part(message, pending, tool_call_index_map);
        *pending_part = Some(part);
    }
    Ok(())
}

fn is_aborted(options: Option<&GenerateOptions>) -> bool {
    options
        .and_then(|options| options.signal.as_ref())
        .is_some_and(|signal| signal.is_cancelled())
}

async fn throw_if_aborted(
    options: Option<&GenerateOptions>,
    stream: Option<&mut dyn StreamedMessage>,
) -> Result<(), ProviderError> {
    if !is_aborted(options) {
        return Ok(());
    }
    if let Some(stream) = stream {
        stream.cancel().await;
    }
    Err(Box::new(create_abort_error()))
}

fn is_pending_tool_call_at_index(
    pending: Option<&StreamedMessagePart>,
    index: &StreamIndex,
) -> bool {
    matches!(
        pending,
        Some(StreamedMessagePart::ToolCall(tool_call))
            if tool_call.stream_index.as_ref() == Some(index)
    )
}

fn flush_part(
    message: &mut Message,
    part: StreamedMessagePart,
    tool_call_index_map: &mut HashMap<StreamIndex, usize>,
) {
    match part {
        StreamedMessagePart::Content(content) => message.content.push(content),
        StreamedMessagePart::ToolCall(mut tool_call) => {
            let stream_index = tool_call.stream_index.take();
            let ordinal = message.tool_calls.len();
            message.tool_calls.push(tool_call);
            if let Some(stream_index) = stream_index {
                tool_call_index_map.insert(stream_index, ordinal);
            }
        }
        StreamedMessagePart::ToolCallPart(_) => {}
    }
}

fn empty_response_error(
    prefix: &str,
    provider: &dyn ChatProvider,
    stream: &dyn StreamedMessage,
) -> ChatProviderError {
    ChatProviderError::empty_response(
        format!(
            "{prefix}{} Provider: {}, model: {}",
            format_finish_reason_hint(stream),
            provider.name(),
            provider.model_name()
        ),
        stream.finish_reason(),
        stream.raw_finish_reason().map(str::to_owned),
    )
}

fn format_finish_reason_hint(stream: &dyn StreamedMessage) -> String {
    if stream.finish_reason().is_none() && stream.raw_finish_reason().is_none() {
        return String::new();
    }
    let finish_reason = stream
        .finish_reason()
        .map(finish_reason_name)
        .unwrap_or("unknown");
    let raw = stream
        .raw_finish_reason()
        .map(|raw| format!(", rawFinishReason={raw}"))
        .unwrap_or_default();
    let filtered_hint = if stream.finish_reason() == Some(FinishReason::Filtered) {
        " The provider filtered the response before visible output was emitted."
    } else {
        ""
    };
    format!(" Provider stop details: finishReason={finish_reason}{raw}.{filtered_hint}")
}

fn finish_reason_name(reason: FinishReason) -> &'static str {
    match reason {
        FinishReason::Completed => "completed",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::Truncated => "truncated",
        FinishReason::Filtered => "filtered",
        FinishReason::Paused => "paused",
        FinishReason::Other => "other",
    }
}

fn owned_trace_id(trace_id: TraceId<'_>) -> Option<Option<String>> {
    match trace_id {
        TraceId::Absent => None,
        TraceId::Present(trace_id) => Some(trace_id.map(str::to_owned)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures_util::Stream;
    use serde_json::Map;
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll};
    use tokio_util::sync::CancellationToken;

    #[derive(Clone)]
    enum TraceState {
        Absent,
        Null,
        Value(String),
    }

    struct FakeStream {
        parts: VecDeque<Result<StreamedMessagePart, ProviderError>>,
        trace: TraceState,
        abort_before_index: Option<(usize, CancellationToken)>,
        yielded: usize,
        cancel_calls: Arc<AtomicUsize>,
    }

    impl FakeStream {
        fn new(parts: Vec<StreamedMessagePart>) -> (Self, Arc<AtomicUsize>) {
            let cancel_calls = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    parts: parts.into_iter().map(Ok).collect(),
                    trace: TraceState::Absent,
                    abort_before_index: None,
                    yielded: 0,
                    cancel_calls: Arc::clone(&cancel_calls),
                },
                cancel_calls,
            )
        }
    }

    impl Stream for FakeStream {
        type Item = Result<StreamedMessagePart, ProviderError>;

        fn poll_next(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            if let Some((index, signal)) = &self.abort_before_index
                && self.yielded == *index
            {
                signal.cancel();
            }
            self.yielded += 1;
            Poll::Ready(self.parts.pop_front())
        }
    }

    impl StreamedMessage for FakeStream {
        fn id(&self) -> Option<&str> {
            Some("gen-1")
        }

        fn usage(&self) -> Option<&TokenUsage> {
            static USAGE: TokenUsage = TokenUsage {
                input_other: 10.0,
                output: 5.0,
                input_cache_read: 2.0,
                input_cache_creation: 1.0,
            };
            Some(&USAGE)
        }

        fn finish_reason(&self) -> Option<FinishReason> {
            Some(FinishReason::Completed)
        }

        fn raw_finish_reason(&self) -> Option<&str> {
            Some("stop")
        }

        fn trace_id(&self) -> TraceId<'_> {
            match &self.trace {
                TraceState::Absent => TraceId::Absent,
                TraceState::Null => TraceId::Present(None),
                TraceState::Value(value) => TraceId::Present(Some(value)),
            }
        }

        fn cancel(
            &mut self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            self.cancel_calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        }
    }

    struct FakeProvider {
        stream: Mutex<Option<FakeStream>>,
        generate_calls: AtomicUsize,
        sent_tool_names: Mutex<Vec<String>>,
        observed_cache_key: Mutex<Option<String>>,
    }

    impl FakeProvider {
        fn new(stream: FakeStream) -> Self {
            Self {
                stream: Mutex::new(Some(stream)),
                generate_calls: AtomicUsize::new(0),
                sent_tool_names: Mutex::new(Vec::new()),
                observed_cache_key: Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl ChatProvider for FakeProvider {
        fn name(&self) -> &str {
            "fake"
        }

        fn model_name(&self) -> &str {
            "fake-model"
        }

        fn thinking_effort(&self) -> Option<&super::super::provider::ThinkingEffort> {
            None
        }

        fn max_completion_tokens(&self) -> Option<f64> {
            None
        }

        async fn generate(
            &self,
            _system_prompt: &str,
            tools: &[Tool],
            _history: &[Message],
            options: Option<&GenerateOptions>,
        ) -> Result<Box<dyn StreamedMessage>, ProviderError> {
            self.generate_calls.fetch_add(1, Ordering::SeqCst);
            *self.sent_tool_names.lock().unwrap() =
                tools.iter().map(|tool| tool.name.clone()).collect();
            *self.observed_cache_key.lock().unwrap() =
                options.and_then(|options| options.cache_key.clone());
            Ok(Box::new(self.stream.lock().unwrap().take().unwrap()))
        }
    }

    fn text(value: &str) -> StreamedMessagePart {
        StreamedMessagePart::Content(ContentPart::Text {
            text: value.to_owned(),
        })
    }

    fn think(value: &str) -> StreamedMessagePart {
        StreamedMessagePart::Content(ContentPart::Think {
            think: value.to_owned(),
            encrypted: None,
        })
    }

    fn tool_call(id: &str, name: &str, arguments: Option<&str>, index: i64) -> StreamedMessagePart {
        StreamedMessagePart::ToolCall(ToolCall {
            call_type: super::super::message::ToolCallType::Function,
            id: id.to_owned(),
            name: name.to_owned(),
            arguments: arguments.map(str::to_owned),
            extras: None,
            stream_index: Some(StreamIndex::Number(index)),
        })
    }

    fn delta(value: &str, index: i64) -> StreamedMessagePart {
        StreamedMessagePart::ToolCallPart(super::super::message::ToolCallPart {
            part_type: super::super::message::ToolCallPartType::ToolCallPart,
            arguments_part: Some(value.to_owned()),
            index: Some(StreamIndex::Number(index)),
        })
    }

    fn history() -> Vec<Message> {
        vec![super::super::message::create_user_message("hi")]
    }

    #[tokio::test]
    async fn merges_text_and_thinking_deltas_and_returns_final_metadata() {
        let (stream, _) = FakeStream::new(vec![
            think("let me "),
            think("think"),
            text("Hello, "),
            text("world"),
        ]);
        let provider = FakeProvider::new(stream);
        let result = generate(&provider, "system", &[], &history(), None, None)
            .await
            .unwrap();
        assert_eq!(
            result.message.content,
            vec![
                ContentPart::Think {
                    think: "let me think".to_owned(),
                    encrypted: None,
                },
                ContentPart::Text {
                    text: "Hello, world".to_owned(),
                },
            ]
        );
        assert_eq!(result.id.as_deref(), Some("gen-1"));
        assert_eq!(result.usage.unwrap().output, 5.0);
        assert_eq!(result.finish_reason, Some(FinishReason::Completed));
        assert_eq!(result.raw_finish_reason.as_deref(), Some("stop"));
    }

    #[tokio::test]
    async fn routes_interleaved_tool_deltas_by_stream_index() {
        let (stream, _) = FakeStream::new(vec![
            tool_call("call-a", "toolA", None, 0),
            tool_call("call-b", "toolB", Some("{\"y\":2}"), 1),
            delta("{\"x\":", 0),
            delta("1}", 0),
        ]);
        let provider = FakeProvider::new(stream);
        let result = generate(&provider, "system", &[], &history(), None, None)
            .await
            .unwrap();
        assert_eq!(result.message.tool_calls.len(), 2);
        assert_eq!(
            result.message.tool_calls[0].arguments.as_deref(),
            Some("{\"x\":1}")
        );
        assert_eq!(
            result.message.tool_calls[1].arguments.as_deref(),
            Some("{\"y\":2}")
        );
        assert!(
            result
                .message
                .tool_calls
                .iter()
                .all(|call| call.stream_index.is_none())
        );
    }

    #[tokio::test]
    async fn callbacks_receive_copies_then_mutable_final_tool_calls() {
        let (stream, _) = FakeStream::new(vec![
            text("abc"),
            tool_call("call-a", "toolA", Some("{}"), 0),
        ]);
        let provider = FakeProvider::new(stream);
        let seen_parts = Arc::new(Mutex::new(Vec::<StreamedMessagePart>::new()));
        let part_sink = Arc::clone(&seen_parts);
        let callbacks = GenerateCallbacks {
            on_message_part: Some(Arc::new(move |mut part| {
                let part_sink = Arc::clone(&part_sink);
                Box::pin(async move {
                    part_sink.lock().unwrap().push(part.clone());
                    if let StreamedMessagePart::Content(ContentPart::Text { text }) = &mut part {
                        *text = "MUTATED".to_owned();
                    }
                })
            })),
            on_tool_call: Some(Arc::new(|call: &mut ToolCall| {
                Box::pin(async move {
                    call.extras = Some(Map::from_iter([(
                        "seen".to_owned(),
                        serde_json::Value::Bool(true),
                    )]));
                })
            })),
        };
        let result = generate(&provider, "system", &[], &history(), Some(&callbacks), None)
            .await
            .unwrap();
        assert_eq!(seen_parts.lock().unwrap().len(), 2);
        assert_eq!(
            result.message.content,
            vec![ContentPart::Text {
                text: "abc".to_owned()
            }]
        );
        assert_eq!(
            result.message.tool_calls[0].extras.as_ref().unwrap()["seen"],
            true
        );
    }

    #[tokio::test]
    async fn filters_deferred_tools_and_passes_intent_options_by_reference() {
        let (stream, _) = FakeStream::new(vec![text("ok")]);
        let provider = FakeProvider::new(stream);
        let tools = vec![
            Tool {
                name: "visible".to_owned(),
                description: "v".to_owned(),
                parameters: Map::new(),
                deferred: None,
            },
            Tool {
                name: "hidden".to_owned(),
                description: "h".to_owned(),
                parameters: Map::new(),
                deferred: Some(true),
            },
        ];
        let options = GenerateOptions {
            cache_key: Some("session-42".to_owned()),
            ..GenerateOptions::default()
        };
        generate(
            &provider,
            "system",
            &tools,
            &history(),
            None,
            Some(&options),
        )
        .await
        .unwrap();
        assert_eq!(*provider.sent_tool_names.lock().unwrap(), ["visible"]);
        assert_eq!(
            provider.observed_cache_key.lock().unwrap().as_deref(),
            Some("session-42")
        );
    }

    #[tokio::test]
    async fn rejects_empty_and_thinking_only_responses_with_stop_details() {
        for parts in [Vec::new(), vec![think("only thinking")]] {
            let (stream, _) = FakeStream::new(parts);
            let provider = FakeProvider::new(stream);
            let error = generate(&provider, "system", &[], &history(), None, None)
                .await
                .unwrap_err();
            let error = error.downcast_ref::<ChatProviderError>().unwrap();
            assert!(matches!(
                error,
                ChatProviderError::ApiEmptyResponse {
                    finish_reason: Some(FinishReason::Completed),
                    ..
                }
            ));
            assert!(error.message().contains("finishReason=completed"));
            assert!(
                error
                    .message()
                    .contains("Provider: fake, model: fake-model")
            );
        }
    }

    #[tokio::test]
    async fn preserves_absent_null_and_string_trace_states() {
        for (state, expected) in [
            (TraceState::Absent, None),
            (TraceState::Null, Some(None)),
            (
                TraceState::Value("trace-123".to_owned()),
                Some(Some("trace-123".to_owned())),
            ),
        ] {
            let (mut stream, _) = FakeStream::new(vec![text("ok")]);
            stream.trace = state;
            let provider = FakeProvider::new(stream);
            let result = generate(&provider, "system", &[], &history(), None, None)
                .await
                .unwrap();
            assert_eq!(result.trace_id, expected);
        }
    }

    #[tokio::test]
    async fn reports_nonnegative_stream_timing_stats() {
        let (stream, _) = FakeStream::new(vec![text("a"), text("b")]);
        let provider = FakeProvider::new(stream);
        let stats = Arc::new(Mutex::new(None));
        let stats_sink = Arc::clone(&stats);
        let options = GenerateOptions {
            on_stream_end: Some(Arc::new(move |value| {
                *stats_sink.lock().unwrap() = value;
            })),
            ..GenerateOptions::default()
        };
        generate(&provider, "system", &[], &history(), None, Some(&options))
            .await
            .unwrap();
        let stats = stats.lock().unwrap().unwrap();
        assert!(stats.server_decode_ms >= 0.0);
        assert!(stats.client_consume_ms >= 0.0);
    }

    #[tokio::test]
    async fn already_aborted_signal_skips_provider_call() {
        let (stream, _) = FakeStream::new(vec![text("ok")]);
        let provider = FakeProvider::new(stream);
        let signal = CancellationToken::new();
        signal.cancel();
        let options = GenerateOptions {
            signal: Some(signal),
            ..GenerateOptions::default()
        };
        let error = generate(&provider, "system", &[], &history(), None, Some(&options))
            .await
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ChatProviderError>(),
            Some(ChatProviderError::Abort)
        ));
        assert_eq!(provider.generate_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn midstream_abort_cancels_stream_and_returns_standard_abort() {
        let signal = CancellationToken::new();
        let (mut stream, cancel_calls) = FakeStream::new(vec![text("first"), text("second")]);
        stream.abort_before_index = Some((1, signal.clone()));
        let provider = FakeProvider::new(stream);
        let options = GenerateOptions {
            signal: Some(signal),
            ..GenerateOptions::default()
        };
        let error = generate(&provider, "system", &[], &history(), None, Some(&options))
            .await
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ChatProviderError>(),
            Some(ChatProviderError::Abort)
        ));
        assert!(cancel_calls.load(Ordering::SeqCst) > 0);
    }
}
