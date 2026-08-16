use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use indexmap::IndexMap;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::pin::Pin;
use tokio_util::sync::CancellationToken;

use crate::kosong::contract::errors::ChatProviderError;
use crate::kosong::contract::provider::ProviderError;
use crate::kosong::provider::bases::anthropic::anthropic::{
    convert_anthropic_error, convert_anthropic_status_error,
};
use crate::kosong::provider::bases::http_client::append_url_path_segments;
use crate::kosong::provider::bases::sse::{SseFrameDecoder, extract_data};

pub type AnthropicEventStream = Pin<Box<dyn Stream<Item = Result<Value, ProviderError>> + Send>>;

pub enum AnthropicHttpResponse {
    Message(Value),
    Stream(AnthropicEventStream),
}

#[async_trait]
pub trait AnthropicClient: Send + Sync {
    async fn create(
        &self,
        params: Map<String, Value>,
        request_headers: Option<&IndexMap<String, String>>,
        stream: bool,
        signal: Option<&CancellationToken>,
    ) -> Result<AnthropicHttpResponse, ProviderError>;
}

pub struct ReqwestAnthropicClient {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    default_headers: Option<IndexMap<String, String>>,
}

impl ReqwestAnthropicClient {
    pub fn new(
        client: reqwest::Client,
        base_url: String,
        api_key: String,
        default_headers: Option<IndexMap<String, String>>,
    ) -> Self {
        Self {
            client,
            base_url,
            api_key,
            default_headers,
        }
    }
}

#[async_trait]
impl AnthropicClient for ReqwestAnthropicClient {
    async fn create(
        &self,
        params: Map<String, Value>,
        request_headers: Option<&IndexMap<String, String>>,
        stream: bool,
        signal: Option<&CancellationToken>,
    ) -> Result<AnthropicHttpResponse, ProviderError> {
        send_anthropic_request(
            &self.client,
            &self.base_url,
            &self.api_key,
            self.default_headers.as_ref(),
            request_headers,
            params,
            stream,
            signal,
        )
        .await
    }
}

fn boxed(error: ChatProviderError) -> ProviderError {
    Box::new(error)
}

fn build_headers(
    api_key: &str,
    default_headers: Option<&IndexMap<String, String>>,
    request_headers: Option<&IndexMap<String, String>>,
) -> Result<HeaderMap, ChatProviderError> {
    let mut values = IndexMap::from([("anthropic-version".to_owned(), "2023-06-01".to_owned())]);
    if let Some(headers) = default_headers {
        values.extend(
            headers
                .iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), value.clone())),
        );
    }
    values.insert("x-api-key".to_owned(), api_key.to_owned());
    if let Some(headers) = request_headers {
        values.extend(headers.clone());
    }

    let mut result = HeaderMap::new();
    for (name, value) in values {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
            ChatProviderError::ChatProvider {
                message: format!("AnthropicChatProvider: invalid header name: {error}"),
            }
        })?;
        let value =
            HeaderValue::from_str(&value).map_err(|error| ChatProviderError::ChatProvider {
                message: format!("AnthropicChatProvider: invalid header value: {error}"),
            })?;
        result.insert(name, value);
    }
    Ok(result)
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

fn error_message(value: &Value, fallback: &str) -> String {
    value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .or_else(|| value.get("message").and_then(Value::as_str))
        .unwrap_or(fallback)
        .to_owned()
}

// Original: anthropic.ts, AnthropicChatProvider.generate() SDK request.
// reqwest speaks the same Messages endpoint; beta client `betas` are encoded
// as the anthropic-beta header before the JSON body is sent.
#[allow(clippy::too_many_arguments)]
pub async fn send_anthropic_request(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    default_headers: Option<&IndexMap<String, String>>,
    request_headers: Option<&IndexMap<String, String>>,
    mut params: Map<String, Value>,
    stream: bool,
    signal: Option<&CancellationToken>,
) -> Result<AnthropicHttpResponse, ProviderError> {
    let merged_headers = params
        .remove("betas")
        .and_then(|value| value.as_array().cloned())
        .and_then(|betas| {
            let joined = betas
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(",");
            (!joined.is_empty()).then(|| {
                let mut headers = request_headers.cloned().unwrap_or_default();
                headers.insert("anthropic-beta".to_owned(), joined);
                headers
            })
        });
    let headers = merged_headers.as_ref().or(request_headers);
    params.insert("stream".to_owned(), Value::Bool(stream));
    let url = append_url_path_segments(base_url, &["v1", "messages"]).map_err(|error| {
        boxed(ChatProviderError::ChatProvider {
            message: format!("AnthropicChatProvider: invalid base URL: {error}"),
        })
    })?;
    let response = await_or_cancel(
        signal,
        client
            .post(url)
            .headers(build_headers(api_key, default_headers, headers).map_err(boxed)?)
            .json(&params)
            .send(),
    )
    .await
    .map_err(boxed)?
    .map_err(convert_anthropic_error)
    .map_err(boxed)?;
    let status = response.status();
    let response_headers = response.headers().clone();
    if !status.is_success() {
        let body = await_or_cancel(signal, response.text())
            .await
            .map_err(boxed)?
            .map_err(convert_anthropic_error)
            .map_err(boxed)?;
        let parsed = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
        return Err(boxed(convert_anthropic_status_error(
            status.as_u16(),
            &error_message(&parsed, &body),
            &response_headers,
        )));
    }
    if stream {
        let source = response.bytes_stream().map(|result| {
            result
                .map(|bytes| bytes.to_vec())
                .map_err(convert_anthropic_error)
                .map_err(boxed)
        });
        let state = SseState {
            source: Box::pin(source),
            decoder: SseFrameDecoder::new(),
            pending: VecDeque::new(),
            done: false,
            signal: signal.cloned(),
        };
        let stream = futures_util::stream::try_unfold(state, |mut state| async move {
            match state.next_event().await? {
                Some(event) => Ok(Some((event, state))),
                None => Ok(None),
            }
        });
        Ok(AnthropicHttpResponse::Stream(Box::pin(stream)))
    } else {
        let message = await_or_cancel(signal, response.json::<Value>())
            .await
            .map_err(boxed)?
            .map_err(convert_anthropic_error)
            .map_err(boxed)?;
        Ok(AnthropicHttpResponse::Message(message))
    }
}

struct SseState {
    source: Pin<Box<dyn Stream<Item = Result<Vec<u8>, ProviderError>> + Send>>,
    decoder: SseFrameDecoder,
    pending: VecDeque<Value>,
    done: bool,
    signal: Option<CancellationToken>,
}

impl SseState {
    async fn next_event(&mut self) -> Result<Option<Value>, ProviderError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }
            self.parse_complete_events()?;
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }
            if self.done {
                // The stream ended without a trailing blank line; drain the
                // final unterminated frame if one is buffered.
                if let Some(frame) = self.decoder.finish() {
                    self.parse_frame(&frame)?;
                    continue;
                }
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
                Some(Ok(bytes)) => self.decoder.push(&bytes),
                Some(Err(error)) => return Err(error),
                None => self.done = true,
            }
        }
    }

    fn parse_complete_events(&mut self) -> Result<(), ProviderError> {
        while let Some(frame) = self.decoder.next_frame() {
            self.parse_frame(&frame)?;
        }
        Ok(())
    }

    fn parse_frame(&mut self, frame: &[u8]) -> Result<(), ProviderError> {
        let data = extract_data(frame).map_err(|error| {
            boxed(ChatProviderError::ChatProvider {
                message: format!("Anthropic error: invalid UTF-8 in SSE response: {error}"),
            })
        })?;
        if data.is_empty() {
            return Ok(());
        }
        let value: Value = serde_json::from_str(&data).map_err(|error| {
            boxed(ChatProviderError::ChatProvider {
                message: format!("Anthropic error: invalid SSE event: {error}"),
            })
        })?;
        if value.get("type").and_then(Value::as_str) == Some("error") {
            let message = value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("stream error");
            return Err(boxed(ChatProviderError::ChatProvider {
                message: format!("Anthropic error: {message}"),
            }));
        }
        self.pending.push_back(value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_fragmented_anthropic_sse_events() {
        let source = futures_util::stream::iter([
            Ok(b"event: content_block_delta\r\ndata: {\"type\":\"content_block_".to_vec()),
            Ok(b"delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\r\n\r\n".to_vec()),
        ]);
        let mut state = SseState {
            source: Box::pin(source),
            decoder: SseFrameDecoder::new(),
            pending: VecDeque::new(),
            done: false,
            signal: None,
        };
        let event = state.next_event().await.unwrap().unwrap();
        assert_eq!(event["delta"]["text"], "hi");
        assert!(state.next_event().await.unwrap().is_none());
    }
}
