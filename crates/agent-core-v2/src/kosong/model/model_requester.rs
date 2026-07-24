//! Per-turn model request contract and streamed request events.
//!
//! Original: `packages/agent-core-v2/src/kosong/model/modelRequester.ts`.

use std::{error::Error, fmt, future::Future, pin::Pin, sync::Arc};

use futures_util::stream::BoxStream;
use tokio_util::sync::CancellationToken;

use crate::kosong::contract::{
    errors::ChatProviderError,
    message::{ContentPart, Message, StreamedMessagePart},
    provider::{
        FinishReason, ProviderError, ResponseFormat, SamplingOptions, ThinkingEffort,
        TraceIdCallback, VideoUploadSource,
    },
    tool::Tool,
    usage::TokenUsage,
};

use crate::_base::errors::errors::Error2;

use super::catalog::Model;

#[derive(Clone)]
pub struct ModelRequestInput {
    pub system_prompt: String,
    pub tools: Vec<Tool>,
    pub messages: Vec<Message>,
    pub response_format: Option<ResponseFormat>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelRequestTiming {
    pub first_token_latency_ms: f64,
    pub stream_duration_ms: f64,
    pub request_build_ms: Option<f64>,
    pub server_first_token_ms: Option<f64>,
    pub server_decode_ms: Option<f64>,
    pub client_consume_ms: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModelRequestEvent {
    Part(StreamedMessagePart),
    Usage {
        usage: TokenUsage,
        model: Option<String>,
    },
    Finish {
        message: Message,
        provider_finish_reason: Option<FinishReason>,
        raw_finish_reason: Option<String>,
        id: Option<String>,
        trace_id: Option<String>,
    },
    Timing(ModelRequestTiming),
}

#[derive(Clone, Debug)]
pub enum ModelRequestError {
    Abort(ChatProviderError),
    Coded(Error2),
}

impl fmt::Display for ModelRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Abort(error) => error.fmt(formatter),
            Self::Coded(error) => error.fmt(formatter),
        }
    }
}

impl Error for ModelRequestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Abort(error) => Some(error),
            Self::Coded(error) => Some(error),
        }
    }
}

#[derive(Clone, Default)]
pub struct ModelRequestParams {
    pub cache_key: Option<String>,
    pub sampling: Option<SamplingOptions>,
    pub thinking_effort: Option<ThinkingEffort>,
    pub thinking_keep: Option<String>,
    pub max_completion_tokens: Option<f64>,
    pub used_context_tokens: Option<f64>,
    pub max_context_tokens: Option<f64>,
    pub on_trace_id: Option<TraceIdCallback>,
}

pub type ModelRequestStream = BoxStream<'static, Result<ModelRequestEvent, ModelRequestError>>;
pub type UploadVideoFuture =
    Pin<Box<dyn Future<Output = Result<Option<ContentPart>, ProviderError>> + Send + 'static>>;

pub trait ModelRequester: Send + Sync {
    // Original: ModelRequester.model.
    fn model(&self) -> Arc<Model>;

    // Original: ModelRequester.request(). An AsyncIterable that throws maps
    // to a fallible Stream; values stay in the source's event order.
    fn request(
        &self,
        input: ModelRequestInput,
        signal: Option<CancellationToken>,
        params: Option<ModelRequestParams>,
    ) -> ModelRequestStream;

    // Original: optional ModelRequester.uploadVideo(). `None` preserves the
    // unsupported capability without conflating it with a provider failure.
    fn upload_video(
        &self,
        _input: VideoUploadSource,
        _signal: Option<CancellationToken>,
    ) -> UploadVideoFuture {
        Box::pin(async { Ok(None) })
    }
}

// Original: effectiveMaxCompletionTokens().
pub fn effective_max_completion_tokens(params: Option<&ModelRequestParams>) -> Option<f64> {
    params.and_then(|params| params.max_completion_tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_budget_returns_only_the_explicit_request_parameter() {
        assert_eq!(effective_max_completion_tokens(None), None);
        assert_eq!(
            effective_max_completion_tokens(Some(&ModelRequestParams {
                max_completion_tokens: Some(4096.5),
                used_context_tokens: Some(1.0),
                max_context_tokens: Some(2.0),
                ..ModelRequestParams::default()
            })),
            Some(4096.5)
        );
    }

    #[test]
    fn request_timing_keeps_all_optional_metrics_separate() {
        let timing = ModelRequestTiming {
            first_token_latency_ms: 12.0,
            stream_duration_ms: 50.0,
            request_build_ms: Some(1.0),
            server_first_token_ms: None,
            server_decode_ms: Some(20.0),
            client_consume_ms: Some(17.0),
        };
        assert_eq!(timing.first_token_latency_ms, 12.0);
        assert_eq!(timing.server_first_token_ms, None);
        assert_eq!(timing.client_consume_ms, Some(17.0));
    }
}
