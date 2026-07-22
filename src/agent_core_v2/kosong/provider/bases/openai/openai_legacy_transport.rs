use futures_util::{Stream, StreamExt};
use indexmap::IndexMap;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::pin::Pin;
use tokio_util::sync::CancellationToken;

use crate::agent_core_v2::kosong::contract::errors::{
    ChatProviderError, classify_base_api_error, normalize_api_status_error, parse_retry_after_ms,
    parse_trace_id,
};
use crate::agent_core_v2::kosong::contract::message::StreamIndex;
use crate::agent_core_v2::kosong::contract::provider::ProviderError;
use crate::agent_core_v2::kosong::provider::bases::openai::chat_completions_stream::{
    ChatCompletionStreamToolCallDelta, ChatCompletionStreamToolFunctionDelta,
};
use crate::agent_core_v2::kosong::provider::bases::openai::openai_legacy::{
    OpenAiLegacyChoice, OpenAiLegacyChunk, OpenAiLegacyCompletion, OpenAiLegacyDelta,
    OpenAiLegacyFunctionCall, OpenAiLegacyMessagePayload, OpenAiLegacyToolCall,
};

pub type OpenAiLegacyChunkStream =
    Pin<Box<dyn Stream<Item = Result<OpenAiLegacyChunk, ProviderError>> + Send>>;

pub enum OpenAiLegacyHttpResponse {
    Completion {
        value: OpenAiLegacyCompletion,
        trace_id: Option<String>,
    },
    Stream {
        value: OpenAiLegacyChunkStream,
        trace_id: Option<String>,
    },
}

fn transport_error(error: reqwest::Error) -> ChatProviderError {
    if error.is_timeout() {
        ChatProviderError::timeout(error.to_string())
    } else if error.is_connect() || error.is_request() || error.is_body() {
        ChatProviderError::connection(error.to_string())
    } else {
        classify_base_api_error(&error.to_string())
    }
}

fn boxed(error: ChatProviderError) -> ProviderError {
    Box::new(error)
}

fn build_headers(
    api_key: &str,
    headers: Option<&IndexMap<String, String>>,
) -> Result<HeaderMap, ChatProviderError> {
    let mut result = HeaderMap::new();
    let authorization = HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|error| {
        ChatProviderError::ChatProvider {
            message: format!("OpenAILegacyChatProvider: invalid apiKey header: {error}"),
        }
    })?;
    result.insert(AUTHORIZATION, authorization);
    if let Some(headers) = headers {
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                ChatProviderError::ChatProvider {
                    message: format!("OpenAILegacyChatProvider: invalid header name: {error}"),
                }
            })?;
            let value =
                HeaderValue::from_str(value).map_err(|error| ChatProviderError::ChatProvider {
                    message: format!("OpenAILegacyChatProvider: invalid header value: {error}"),
                })?;
            result.insert(name, value);
        }
    }
    Ok(result)
}

fn response_error_message(value: &Value, fallback: &str) -> String {
    value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .or_else(|| value.get("message").and_then(Value::as_str))
        .unwrap_or(fallback)
        .to_owned()
}

async fn await_or_cancel<T>(
    signal: Option<&CancellationToken>,
    future: impl Future<Output = T>,
) -> Result<T, ChatProviderError> {
    if let Some(signal) = signal {
        tokio::select! {
            biased;
            _ = signal.cancelled() => Err(ChatProviderError::Abort),
            value = future => Ok(value),
        }
    } else {
        Ok(future.await)
    }
}

// Original:
//   openai-legacy.ts, OpenAILegacyChatProvider.generate() SDK request.
//
// Rust adaptation:
//   reqwest replaces the OpenAI JavaScript SDK. Headers resolve before this
//   call, while this function preserves response-header availability before
//   an SSE body is consumed and maps transport/status errors into the shared
//   contract error family.
#[allow(clippy::too_many_arguments)]
pub async fn send_openai_legacy_request(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    headers: Option<&IndexMap<String, String>>,
    params: Map<String, Value>,
    stream: bool,
    signal: Option<&CancellationToken>,
) -> Result<OpenAiLegacyHttpResponse, ProviderError> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let request = client
        .post(url)
        .headers(build_headers(api_key, headers).map_err(boxed)?)
        .json(&params);
    let response = await_or_cancel(signal, request.send())
        .await
        .map_err(boxed)?
        .map_err(transport_error)
        .map_err(boxed)?;
    let status = response.status();
    let response_headers = response.headers().clone();
    let trace_id = parse_trace_id(Some(&response_headers));

    if !status.is_success() {
        let body = await_or_cancel(signal, response.text())
            .await
            .map_err(boxed)?
            .map_err(transport_error)
            .map_err(boxed)?;
        let parsed = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
        let message = response_error_message(&parsed, &body);
        let request_id = response_headers
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        return Err(boxed(normalize_api_status_error(
            i32::from(status.as_u16()),
            &message,
            request_id,
            parse_retry_after_ms(Some(&response_headers)),
            trace_id,
        )));
    }

    if stream {
        let bytes = response.bytes_stream().map(|result| {
            result
                .map(|bytes| bytes.to_vec())
                .map_err(transport_error)
                .map_err(boxed)
        });
        let state = SseState {
            source: Box::pin(bytes),
            buffer: Vec::new(),
            pending: VecDeque::new(),
            done: false,
            signal: signal.cloned(),
        };
        let value = futures_util::stream::try_unfold(state, |mut state| async move {
            match state.next_chunk().await? {
                Some(chunk) => Ok(Some((chunk, state))),
                None => Ok(None),
            }
        });
        Ok(OpenAiLegacyHttpResponse::Stream {
            value: Box::pin(value),
            trace_id,
        })
    } else {
        let value = await_or_cancel(signal, response.json::<Value>())
            .await
            .map_err(boxed)?
            .map_err(transport_error)
            .map_err(boxed)
            .and_then(parse_completion)?;
        Ok(OpenAiLegacyHttpResponse::Completion { value, trace_id })
    }
}

struct SseState {
    source: Pin<Box<dyn Stream<Item = Result<Vec<u8>, ProviderError>> + Send>>,
    buffer: Vec<u8>,
    pending: VecDeque<OpenAiLegacyChunk>,
    done: bool,
    signal: Option<CancellationToken>,
}

impl SseState {
    async fn next_chunk(&mut self) -> Result<Option<OpenAiLegacyChunk>, ProviderError> {
        loop {
            if let Some(chunk) = self.pending.pop_front() {
                return Ok(Some(chunk));
            }
            self.parse_complete_events()?;
            if let Some(chunk) = self.pending.pop_front() {
                return Ok(Some(chunk));
            }
            if self.done {
                return Ok(None);
            }
            let next = if let Some(signal) = self.signal.as_ref() {
                tokio::select! {
                    biased;
                    _ = signal.cancelled() => return Err(boxed(ChatProviderError::Abort)),
                    next = self.source.next() => next,
                }
            } else {
                self.source.next().await
            };
            match next {
                Some(Ok(bytes)) => self.buffer.extend(bytes),
                Some(Err(error)) => return Err(error),
                None => {
                    self.done = true;
                    if !self.buffer.is_empty() {
                        self.buffer.extend_from_slice(b"\n\n");
                    }
                }
            }
        }
    }

    fn parse_complete_events(&mut self) -> Result<(), ProviderError> {
        while let Some((at, delimiter_len)) = find_event_boundary(&self.buffer) {
            let event = self.buffer.drain(..at).collect::<Vec<_>>();
            self.buffer.drain(..delimiter_len);
            let text = std::str::from_utf8(&event).map_err(|error| {
                boxed(ChatProviderError::ChatProvider {
                    message: format!("Error: invalid UTF-8 in OpenAI SSE response: {error}"),
                })
            })?;
            let data = text
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim_start)
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() {
                continue;
            }
            if data.trim() == "[DONE]" {
                self.done = true;
                self.buffer.clear();
                break;
            }
            let value = serde_json::from_str::<Value>(&data).map_err(|error| {
                boxed(ChatProviderError::ChatProvider {
                    message: format!("Error: invalid OpenAI SSE event: {error}"),
                })
            })?;
            self.pending.push_back(parse_chunk(value)?);
        }
        Ok(())
    }
}

fn find_event_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(lf), Some(crlf)) if lf < crlf => Some((lf, 2)),
        (Some(_), Some(crlf)) => Some((crlf, 4)),
        (Some(lf), None) => Some((lf, 2)),
        (None, Some(crlf)) => Some((crlf, 4)),
        (None, None) => None,
    }
}

fn object(value: Value, label: &str) -> Result<Map<String, Value>, ProviderError> {
    value.as_object().cloned().ok_or_else(|| {
        boxed(ChatProviderError::ChatProvider {
            message: format!("Error: invalid OpenAI {label} response"),
        })
    })
}

fn parse_function(value: &Value) -> OpenAiLegacyFunctionCall {
    OpenAiLegacyFunctionCall {
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        arguments: value
            .get("arguments")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

fn parse_message(value: &Value) -> OpenAiLegacyMessagePayload {
    let fields = value.as_object().cloned().unwrap_or_default();
    let tool_calls = value
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|call| OpenAiLegacyToolCall {
            call_type: call
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            id: call
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            function: parse_function(call.get("function").unwrap_or(&Value::Null)),
        })
        .collect();
    OpenAiLegacyMessagePayload {
        fields,
        content: value
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_owned),
        tool_calls,
    }
}

fn parse_delta(value: &Value) -> OpenAiLegacyDelta {
    let fields = value.as_object().cloned().unwrap_or_default();
    let tool_calls = value
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|call| ChatCompletionStreamToolCallDelta {
            index: call
                .get("index")
                .cloned()
                .and_then(|index| serde_json::from_value::<StreamIndex>(index).ok()),
            id: call.get("id").and_then(Value::as_str).map(str::to_owned),
            function: call
                .get("function")
                .map(|function| ChatCompletionStreamToolFunctionDelta {
                    name: function
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    arguments: function
                        .get("arguments")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                }),
        })
        .collect();
    OpenAiLegacyDelta {
        fields,
        content: value
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_owned),
        tool_calls,
    }
}

fn parse_choices(raw: &Map<String, Value>) -> Vec<OpenAiLegacyChoice> {
    raw.get("choices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|choice| OpenAiLegacyChoice {
            finish_reason: choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .map(str::to_owned),
            message: choice.get("message").map(parse_message),
            delta: choice.get("delta").map(parse_delta),
        })
        .collect()
}

pub fn parse_completion(value: Value) -> Result<OpenAiLegacyCompletion, ProviderError> {
    let raw = object(value, "completion")?;
    Ok(OpenAiLegacyCompletion {
        id: raw
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        usage: raw.get("usage").cloned(),
        choices: parse_choices(&raw),
        raw,
    })
}

pub fn parse_chunk(value: Value) -> Result<OpenAiLegacyChunk, ProviderError> {
    let raw = object(value, "stream")?;
    Ok(OpenAiLegacyChunk {
        id: raw.get("id").and_then(Value::as_str).map(str::to_owned),
        usage: raw.get("usage").cloned(),
        choices: parse_choices(&raw),
        raw,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_fragmented_sse_and_non_stream_payloads() {
        let source = futures_util::stream::iter([
            Ok(b"data: {\"id\":\"chat-1\",\"choices\":[{\"delta\":{\"content\":\"he".to_vec()),
            Ok(b"llo\"},\"finish_reason\":null}]}\r\n\r\ndata: [DONE]\r\n\r\n".to_vec()),
        ]);
        let mut state = SseState {
            source: Box::pin(source),
            buffer: Vec::new(),
            pending: VecDeque::new(),
            done: false,
            signal: None,
        };
        let chunk = state.next_chunk().await.unwrap().unwrap();
        assert_eq!(chunk.id.as_deref(), Some("chat-1"));
        assert_eq!(
            chunk.choices[0].delta.as_ref().unwrap().content.as_deref(),
            Some("hello")
        );
        assert!(state.next_chunk().await.unwrap().is_none());

        let completion = parse_completion(serde_json::json!({
            "id":"chat-2",
            "usage":{"prompt_tokens":1,"completion_tokens":2},
            "choices":[{"finish_reason":"stop","message":{"content":"done"}}]
        }))
        .unwrap();
        assert_eq!(completion.id, "chat-2");
        assert_eq!(completion.choices[0].finish_reason.as_deref(), Some("stop"));
    }
}
