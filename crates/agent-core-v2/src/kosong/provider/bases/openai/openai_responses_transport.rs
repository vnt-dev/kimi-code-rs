use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use indexmap::IndexMap;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::pin::Pin;
use tokio_util::sync::CancellationToken;

use crate::kosong::contract::errors::ChatProviderError;
use crate::kosong::contract::provider::ProviderError;
use crate::kosong::provider::bases::http_client::append_url_path_segments;
use crate::kosong::provider::bases::openai::openai_common::{
    convert_openai_error, convert_openai_status_error,
};
use crate::kosong::provider::bases::sse::{SseFrameDecoder, extract_data};

pub type OpenAiResponsesValueStream =
    Pin<Box<dyn Stream<Item = Result<Value, ProviderError>> + Send>>;

pub enum OpenAiResponsesHttpResponse {
    Response(Value),
    Stream(OpenAiResponsesValueStream),
}

#[async_trait]
pub trait OpenAiResponsesClient: Send + Sync {
    async fn create(
        &self,
        params: Map<String, Value>,
        stream: bool,
        signal: Option<&CancellationToken>,
    ) -> Result<OpenAiResponsesHttpResponse, ProviderError>;
}

pub struct ReqwestOpenAiResponsesClient {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    headers: Option<IndexMap<String, String>>,
}

impl ReqwestOpenAiResponsesClient {
    pub fn new(
        client: reqwest::Client,
        base_url: String,
        api_key: String,
        headers: Option<IndexMap<String, String>>,
    ) -> Self {
        Self {
            client,
            base_url,
            api_key,
            headers,
        }
    }
}

#[async_trait]
impl OpenAiResponsesClient for ReqwestOpenAiResponsesClient {
    async fn create(
        &self,
        params: Map<String, Value>,
        stream: bool,
        signal: Option<&CancellationToken>,
    ) -> Result<OpenAiResponsesHttpResponse, ProviderError> {
        send_openai_responses_request(
            &self.client,
            &self.base_url,
            &self.api_key,
            self.headers.as_ref(),
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
    headers: Option<&IndexMap<String, String>>,
) -> Result<HeaderMap, ChatProviderError> {
    let mut result = HeaderMap::new();
    let authorization = HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|error| {
        ChatProviderError::ChatProvider {
            message: format!("OpenAIResponsesChatProvider: invalid apiKey header: {error}"),
        }
    })?;
    result.insert(AUTHORIZATION, authorization);
    if let Some(headers) = headers {
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                ChatProviderError::ChatProvider {
                    message: format!("OpenAIResponsesChatProvider: invalid header name: {error}"),
                }
            })?;
            let value =
                HeaderValue::from_str(value).map_err(|error| ChatProviderError::ChatProvider {
                    message: format!("OpenAIResponsesChatProvider: invalid header value: {error}"),
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

// Original: openai-responses.ts, client.responses.create().
//
// Rust adaptation: reqwest performs the native `/responses` JSON/SSE request.
// Authentication, cancellation and status normalization retain the SDK call's
// observable boundary while request shaping stays in openai_responses.rs.
#[allow(clippy::too_many_arguments)]
pub async fn send_openai_responses_request(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    headers: Option<&IndexMap<String, String>>,
    params: Map<String, Value>,
    stream: bool,
    signal: Option<&CancellationToken>,
) -> Result<OpenAiResponsesHttpResponse, ProviderError> {
    let url = append_url_path_segments(base_url, &["responses"]).map_err(|error| {
        boxed(ChatProviderError::ChatProvider {
            message: format!("OpenAIResponsesChatProvider: invalid base URL: {error}"),
        })
    })?;
    let request = client
        .post(url)
        .headers(build_headers(api_key, headers).map_err(boxed)?)
        .json(&params);
    let response = await_or_cancel(signal, request.send())
        .await
        .map_err(boxed)?
        .map_err(convert_openai_error)
        .map_err(boxed)?;
    let status = response.status();
    let response_headers = response.headers().clone();
    if !status.is_success() {
        let body = await_or_cancel(signal, response.text())
            .await
            .map_err(boxed)?
            .map_err(convert_openai_error)
            .map_err(boxed)?;
        let parsed = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
        return Err(boxed(convert_openai_status_error(
            status.as_u16(),
            &response_error_message(&parsed, &body),
            &response_headers,
        )));
    }

    if !stream {
        let response = await_or_cancel(signal, response.json::<Value>())
            .await
            .map_err(boxed)?
            .map_err(convert_openai_error)
            .map_err(boxed)?;
        return Ok(OpenAiResponsesHttpResponse::Response(response));
    }

    let bytes = response.bytes_stream().map(|result| {
        result
            .map(|bytes| bytes.to_vec())
            .map_err(convert_openai_error)
            .map_err(boxed)
    });
    let state = ResponsesSseState {
        source: Box::pin(bytes),
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
    Ok(OpenAiResponsesHttpResponse::Stream(Box::pin(stream)))
}

struct ResponsesSseState {
    source: Pin<Box<dyn Stream<Item = Result<Vec<u8>, ProviderError>> + Send>>,
    decoder: SseFrameDecoder,
    pending: VecDeque<Value>,
    done: bool,
    signal: Option<CancellationToken>,
}

impl ResponsesSseState {
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
                message: format!("Error: invalid UTF-8 in OpenAI SSE response: {error}"),
            })
        })?;
        if data.is_empty() {
            return Ok(());
        }
        if data.trim() == "[DONE]" {
            self.done = true;
            self.decoder.clear();
            return Ok(());
        }
        let value = serde_json::from_str(&data).map_err(|error| {
            boxed(ChatProviderError::ChatProvider {
                message: format!("Error: invalid OpenAI SSE event: {error}"),
            })
        })?;
        self.pending.push_back(value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_fragmented_responses_sse() {
        let source = futures_util::stream::iter([
            Ok(b"event: response.output_text.delta\r\ndata: {\"type\":\"response.output_text.delta\",\"del"
                .to_vec()),
            Ok(b"ta\":\"hello\"}\r\n\r\ndata: [DONE]\r\n\r\n".to_vec()),
        ]);
        let mut state = ResponsesSseState {
            source: Box::pin(source),
            decoder: SseFrameDecoder::new(),
            pending: VecDeque::new(),
            done: false,
            signal: None,
        };
        let event = state.next_event().await.unwrap().unwrap();
        assert_eq!(event["type"], "response.output_text.delta");
        assert_eq!(event["delta"], "hello");
        assert!(state.next_event().await.unwrap().is_none());
    }
}
