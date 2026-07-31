use async_trait::async_trait;
use futures_util::Stream;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use super::message::{ContentPart, Message, StreamedMessagePart};
use super::tool::Tool;
use super::usage::TokenUsage;

// Original:
//   packages/agent-core-v2/src/kosong/contract/provider.ts
//   ThinkingEffort
//
// Rust adaptation:
//   The TypeScript union deliberately accepts arbitrary provider-defined
//   strings. A transparent newtype preserves that open string contract while
//   preventing unrelated strings from being passed accidentally.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThinkingEffort(String);

impl ThinkingEffort {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_off(&self) -> bool {
        self.0 == "off"
    }
}

impl From<&str> for ThinkingEffort {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for ThinkingEffort {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl AsRef<str> for ThinkingEffort {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ThinkingEffort {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

pub type JsonSchemaObject = Map<String, Value>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonSchemaDefinition {
    pub name: String,
    pub schema: JsonSchemaObject,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// Original: provider.ts, ResponseFormat
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ResponseFormat {
    #[serde(rename = "json_object")]
    JsonObject,
    #[serde(rename = "json_schema")]
    JsonSchema {
        #[serde(rename = "jsonSchema")]
        json_schema: JsonSchemaDefinition,
    },
}

// Original: provider.ts, FinishReason
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Completed,
    ToolCalls,
    Truncated,
    Filtered,
    Paused,
    Other,
}

pub type ProviderError = Box<dyn Error + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceId<'a> {
    Absent,
    Present(Option<&'a str>),
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/provider.ts
//   StreamedMessage
//
// Rust adaptation:
//   AsyncIterator becomes a fallible Stream. Metadata remains on the stream
//   object and may change as provider events are consumed, matching the
//   original implementations' getter-backed fields.
pub trait StreamedMessage:
    Stream<Item = Result<StreamedMessagePart, ProviderError>> + Send + Unpin
{
    fn id(&self) -> Option<&str>;
    fn usage(&self) -> Option<&TokenUsage>;
    fn finish_reason(&self) -> Option<FinishReason>;
    fn raw_finish_reason(&self) -> Option<&str>;
    fn trace_id(&self) -> TraceId<'_>;

    fn cancel(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRequestAuth {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<IndexMap<String, String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SamplingOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingRequestOptions {
    pub effort: ThinkingEffort,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep: Option<String>,
}

pub type ToolCallIdNormalizer = Arc<dyn Fn(&str) -> String + Send + Sync>;

// Original: provider.ts, ToolCallIdPolicy
#[derive(Clone)]
pub struct ToolCallIdPolicy {
    normalize: ToolCallIdNormalizer,
    pub max_length: Option<usize>,
}

impl ToolCallIdPolicy {
    pub fn new(normalize: ToolCallIdNormalizer, max_length: Option<usize>) -> Self {
        Self {
            normalize,
            max_length,
        }
    }

    pub fn normalize(&self, id: &str) -> String {
        (self.normalize)(id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamDecodeStats {
    pub server_decode_ms: f64,
    pub client_consume_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoUploadInput {
    pub data: Vec<u8>,
    pub mime_type: String,
    pub filename: Option<String>,
}

#[derive(Clone)]
pub enum VideoUploadSource {
    Location(String),
    Data(VideoUploadInput),
}

pub type VoidCallback = Arc<dyn Fn() + Send + Sync>;
pub type StreamEndCallback = Arc<dyn Fn(Option<StreamDecodeStats>) + Send + Sync>;
pub type TraceIdCallback = Arc<dyn Fn(Option<&str>) + Send + Sync>;

// Original: provider.ts, GenerateOptions
//
// Rust adaptation:
//   AbortSignal maps to CancellationToken. Callback Arcs keep options cheap to
//   clone without changing their call order or introducing background tasks.
#[derive(Clone, Default)]
pub struct GenerateOptions {
    pub signal: Option<CancellationToken>,
    pub auth: Option<ProviderRequestAuth>,
    pub response_format: Option<ResponseFormat>,
    pub cache_key: Option<String>,
    pub sampling: Option<SamplingOptions>,
    pub thinking: Option<ThinkingRequestOptions>,
    pub max_completion_tokens: Option<u64>,
    pub used_context_tokens: Option<u64>,
    pub max_context_tokens: Option<u64>,
    pub on_request_start: Option<VoidCallback>,
    pub on_request_sent: Option<VoidCallback>,
    pub on_stream_end: Option<StreamEndCallback>,
    pub on_trace_id: Option<TraceIdCallback>,
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/provider.ts
//   ChatProvider
//
// Rust adaptation:
//   async-trait retains dynamic dispatch, which the model catalog needs.
//   Optional uploadVideo becomes Ok(None) when unsupported; a supported
//   implementation returns Some(ContentPart::VideoUrl { .. }).
#[async_trait]
pub trait ChatProvider: Send + Sync {
    fn name(&self) -> &str;
    fn model_name(&self) -> &str;
    fn thinking_effort(&self) -> Option<&ThinkingEffort>;
    fn max_completion_tokens(&self) -> Option<u64>;

    async fn generate(
        &self,
        system_prompt: &str,
        tools: &[Tool],
        history: &[Message],
        options: Option<&GenerateOptions>,
    ) -> Result<Box<dyn StreamedMessage>, ProviderError>;

    async fn upload_video(
        &self,
        _input: VideoUploadSource,
        _options: Option<&GenerateOptions>,
    ) -> Result<Option<ContentPart>, ProviderError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll};

    #[test]
    fn effort_preserves_open_string_and_json_contract() {
        let effort = ThinkingEffort::from("provider-custom");
        assert_eq!(effort.as_str(), "provider-custom");
        assert_eq!(
            serde_json::to_string(&effort).unwrap(),
            "\"provider-custom\""
        );
        assert_eq!(
            serde_json::from_str::<ThinkingEffort>("\"off\"").unwrap(),
            ThinkingEffort::from("off")
        );
    }

    #[test]
    fn response_formats_and_finish_reasons_preserve_wire_names() {
        assert_eq!(
            serde_json::to_value(ResponseFormat::JsonObject).unwrap(),
            serde_json::json!({"type": "json_object"})
        );
        let schema = ResponseFormat::JsonSchema {
            json_schema: JsonSchemaDefinition {
                name: "answer".to_owned(),
                schema: serde_json::from_value(serde_json::json!({"type": "object"})).unwrap(),
                strict: Some(true),
                description: None,
            },
        };
        assert_eq!(
            serde_json::to_value(schema).unwrap(),
            serde_json::json!({
                "type": "json_schema",
                "jsonSchema": {
                    "name": "answer",
                    "schema": {"type": "object"},
                    "strict": true,
                },
            })
        );
        assert_eq!(
            serde_json::to_string(&FinishReason::ToolCalls).unwrap(),
            "\"tool_calls\""
        );
    }

    #[test]
    fn per_turn_data_contracts_preserve_camel_case() {
        assert_eq!(
            serde_json::to_value(ProviderRequestAuth {
                api_key: Some("secret".to_owned()),
                headers: Some(IndexMap::from([("X-Test".to_owned(), "yes".to_owned())])),
            })
            .unwrap(),
            serde_json::json!({"apiKey":"secret","headers":{"X-Test":"yes"}})
        );
        assert_eq!(
            serde_json::to_value(SamplingOptions {
                temperature: Some(0.7),
                top_p: Some(0.9),
            })
            .unwrap(),
            serde_json::json!({"temperature":0.7,"topP":0.9})
        );
        assert_eq!(
            serde_json::to_value(StreamDecodeStats {
                server_decode_ms: 12.5,
                client_consume_ms: 3.0,
            })
            .unwrap(),
            serde_json::json!({"serverDecodeMs":12.5,"clientConsumeMs":3.0})
        );
    }

    #[test]
    fn policy_and_callbacks_remain_caller_controlled() {
        let policy = ToolCallIdPolicy::new(
            Arc::new(|id| id.chars().filter(char::is_ascii_alphanumeric).collect()),
            Some(16),
        );
        assert_eq!(policy.normalize("call-1/2"), "call12");
        assert_eq!(policy.max_length, Some(16));

        let calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = Arc::clone(&calls);
        let signal = CancellationToken::new();
        let options = GenerateOptions {
            signal: Some(signal.clone()),
            cache_key: Some("session-1".to_owned()),
            max_completion_tokens: Some(8192),
            on_request_start: Some(Arc::new(move || {
                callback_calls.fetch_add(1, Ordering::SeqCst);
            })),
            ..GenerateOptions::default()
        };
        options.on_request_start.as_ref().unwrap()();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(options.cache_key.as_deref(), Some("session-1"));
        assert!(!signal.is_cancelled());
        options.signal.as_ref().unwrap().cancel();
        assert!(signal.is_cancelled());
    }

    struct TestStream {
        parts: VecDeque<Result<StreamedMessagePart, ProviderError>>,
        usage: TokenUsage,
    }

    impl Stream for TestStream {
        type Item = Result<StreamedMessagePart, ProviderError>;

        fn poll_next(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            Poll::Ready(self.parts.pop_front())
        }
    }

    impl StreamedMessage for TestStream {
        fn id(&self) -> Option<&str> {
            Some("response-1")
        }

        fn usage(&self) -> Option<&TokenUsage> {
            Some(&self.usage)
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

    struct TestProvider;

    #[async_trait]
    impl ChatProvider for TestProvider {
        fn name(&self) -> &str {
            "test"
        }

        fn model_name(&self) -> &str {
            "test-model"
        }

        fn thinking_effort(&self) -> Option<&ThinkingEffort> {
            None
        }

        fn max_completion_tokens(&self) -> Option<u64> {
            Some(4096)
        }

        async fn generate(
            &self,
            _system_prompt: &str,
            _tools: &[Tool],
            _history: &[Message],
            _options: Option<&GenerateOptions>,
        ) -> Result<Box<dyn StreamedMessage>, ProviderError> {
            Ok(Box::new(TestStream {
                parts: VecDeque::from([Ok(StreamedMessagePart::Content(ContentPart::Text {
                    text: "hello".to_owned(),
                }))]),
                usage: TokenUsage {
                    output: 1.0,
                    ..TokenUsage::default()
                },
            }))
        }
    }

    #[tokio::test]
    async fn chat_provider_remains_dynamically_dispatchable_and_streaming() {
        let provider: &dyn ChatProvider = &TestProvider;
        assert_eq!(provider.name(), "test");
        assert_eq!(provider.model_name(), "test-model");
        assert_eq!(provider.max_completion_tokens(), Some(4096));
        assert!(
            provider
                .upload_video(VideoUploadSource::Location("video.mp4".to_owned()), None)
                .await
                .unwrap()
                .is_none()
        );

        let mut stream = provider.generate("system", &[], &[], None).await.unwrap();
        assert_eq!(stream.id(), Some("response-1"));
        assert_eq!(stream.trace_id(), TraceId::Present(Some("trace-1")));
        assert_eq!(stream.usage().unwrap().output, 1.0);
        assert_eq!(stream.finish_reason(), Some(FinishReason::Completed));
        let part = stream.next().await.unwrap().unwrap();
        assert!(matches!(
            part,
            StreamedMessagePart::Content(ContentPart::Text { ref text }) if text == "hello"
        ));
        assert!(stream.next().await.is_none());
    }
}
