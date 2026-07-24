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
use crate::kosong::provider::bases::google_genai::google_genai::{
    convert_google_gen_ai_error, convert_google_gen_ai_status_error,
};

pub type GoogleGenAiEventStream = Pin<Box<dyn Stream<Item = Result<Value, ProviderError>> + Send>>;

pub enum GoogleGenAiHttpResponse {
    Response(Value),
    Stream(GoogleGenAiEventStream),
}

#[async_trait]
pub trait GoogleGenAiClient: Send + Sync {
    async fn generate(
        &self,
        params: Map<String, Value>,
        stream: bool,
        signal: Option<&CancellationToken>,
    ) -> Result<GoogleGenAiHttpResponse, ProviderError>;
}

pub struct ReqwestGoogleGenAiClient {
    client: reqwest::Client,
    base_url: Option<String>,
    api_key: Option<String>,
    default_headers: Option<IndexMap<String, String>>,
    vertex: Option<(String, String)>,
}

impl ReqwestGoogleGenAiClient {
    pub fn new(
        client: reqwest::Client,
        base_url: Option<String>,
        api_key: Option<String>,
        default_headers: Option<IndexMap<String, String>>,
        vertex: Option<(String, String)>,
    ) -> Self {
        Self {
            client,
            base_url,
            api_key,
            default_headers,
            vertex,
        }
    }
}

#[async_trait]
impl GoogleGenAiClient for ReqwestGoogleGenAiClient {
    async fn generate(
        &self,
        params: Map<String, Value>,
        stream: bool,
        signal: Option<&CancellationToken>,
    ) -> Result<GoogleGenAiHttpResponse, ProviderError> {
        send_google_gen_ai_request(
            &self.client,
            self.base_url.as_deref(),
            self.api_key.as_deref(),
            self.default_headers.as_ref(),
            self.vertex
                .as_ref()
                .map(|(project, location)| (project.as_str(), location.as_str())),
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

fn headers(
    api_key: Option<&str>,
    defaults: Option<&IndexMap<String, String>>,
) -> Result<HeaderMap, ChatProviderError> {
    let mut result = HeaderMap::new();
    if let Some(defaults) = defaults {
        for (name, value) in defaults {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                ChatProviderError::ChatProvider {
                    message: format!("GoogleGenAIChatProvider: invalid header name: {error}"),
                }
            })?;
            let value =
                HeaderValue::from_str(value).map_err(|error| ChatProviderError::ChatProvider {
                    message: format!("GoogleGenAIChatProvider: invalid header value: {error}"),
                })?;
            result.insert(name, value);
        }
    }
    if let Some(api_key) = api_key {
        result.insert(
            "x-goog-api-key",
            HeaderValue::from_str(api_key).map_err(|error| ChatProviderError::ChatProvider {
                message: format!("GoogleGenAIChatProvider: invalid apiKey header: {error}"),
            })?,
        );
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

fn rest_body(mut params: Map<String, Value>) -> Map<String, Value> {
    params.remove("model");
    let config = params
        .remove("config")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let mut generation_config = Map::new();
    for (key, value) in config {
        match key.as_str() {
            "systemInstruction" => {
                params.insert(
                    "systemInstruction".to_owned(),
                    serde_json::json!({"parts":[{"text":value}]}),
                );
            }
            "tools" => {
                params.insert("tools".to_owned(), value);
            }
            _ => {
                generation_config.insert(key, value);
            }
        }
    }
    if !generation_config.is_empty() {
        params.insert(
            "generationConfig".to_owned(),
            Value::Object(generation_config),
        );
    }
    params
}

fn endpoint(
    base_url: Option<&str>,
    model: &str,
    stream: bool,
    vertex: Option<(&str, &str)>,
) -> String {
    let method = if stream {
        "streamGenerateContent?alt=sse"
    } else {
        "generateContent"
    };
    if let Some((project, location)) = vertex {
        let base = base_url
            .map(str::to_owned)
            .unwrap_or_else(|| format!("https://{location}-aiplatform.googleapis.com"));
        format!(
            "{}/v1/projects/{project}/locations/{location}/publishers/google/models/{model}:{method}",
            base.trim_end_matches('/')
        )
    } else {
        let base = base_url.unwrap_or("https://generativelanguage.googleapis.com");
        format!(
            "{}/v1beta/models/{model}:{method}",
            base.trim_end_matches('/')
        )
    }
}

// Original: google-genai.ts, models.generateContent[Stream]() SDK boundary.
#[allow(clippy::too_many_arguments)]
pub async fn send_google_gen_ai_request(
    client: &reqwest::Client,
    base_url: Option<&str>,
    api_key: Option<&str>,
    default_headers: Option<&IndexMap<String, String>>,
    vertex: Option<(&str, &str)>,
    params: Map<String, Value>,
    stream: bool,
    signal: Option<&CancellationToken>,
) -> Result<GoogleGenAiHttpResponse, ProviderError> {
    let model = params
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let url = endpoint(base_url, model, stream, vertex);
    let response = await_or_cancel(
        signal,
        client
            .post(url)
            .headers(headers(api_key, default_headers).map_err(boxed)?)
            .json(&rest_body(params))
            .send(),
    )
    .await
    .map_err(boxed)?
    .map_err(convert_google_gen_ai_error)
    .map_err(boxed)?;
    let status = response.status();
    if !status.is_success() {
        let body = await_or_cancel(signal, response.text())
            .await
            .map_err(boxed)?
            .map_err(convert_google_gen_ai_error)
            .map_err(boxed)?;
        let value = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
        return Err(boxed(convert_google_gen_ai_status_error(
            status.as_u16(),
            &error_message(&value, &body),
        )));
    }
    if stream {
        let source = response.bytes_stream().map(|result| {
            result
                .map(|bytes| bytes.to_vec())
                .map_err(convert_google_gen_ai_error)
                .map_err(boxed)
        });
        let state = SseState {
            source: Box::pin(source),
            buffer: Vec::new(),
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
        Ok(GoogleGenAiHttpResponse::Stream(Box::pin(stream)))
    } else {
        let value = await_or_cancel(signal, response.json::<Value>())
            .await
            .map_err(boxed)?
            .map_err(convert_google_gen_ai_error)
            .map_err(boxed)?;
        Ok(GoogleGenAiHttpResponse::Response(value))
    }
}

struct SseState {
    source: Pin<Box<dyn Stream<Item = Result<Vec<u8>, ProviderError>> + Send>>,
    buffer: Vec<u8>,
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
            self.parse_events()?;
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
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

    fn parse_events(&mut self) -> Result<(), ProviderError> {
        while let Some((at, delimiter)) = find_boundary(&self.buffer) {
            let event = self.buffer.drain(..at).collect::<Vec<_>>();
            self.buffer.drain(..delimiter);
            let text = std::str::from_utf8(&event).map_err(|error| {
                boxed(ChatProviderError::ChatProvider {
                    message: format!("GoogleGenAI error: invalid UTF-8 in SSE response: {error}"),
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
            let value = serde_json::from_str(&data).map_err(|error| {
                boxed(ChatProviderError::ChatProvider {
                    message: format!("GoogleGenAI error: invalid SSE event: {error}"),
                })
            })?;
            self.pending.push_back(value);
        }
        Ok(())
    }
}

fn find_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn maps_sdk_config_and_parses_fragmented_sse() {
        let body = rest_body(Map::from_iter([
            ("model".to_owned(), Value::String("gemini".to_owned())),
            ("contents".to_owned(), serde_json::json!([])),
            (
                "config".to_owned(),
                serde_json::json!({
                    "systemInstruction":"system",
                    "maxOutputTokens":12,
                    "tools":[{"functionDeclarations":[]}]
                }),
            ),
        ]));
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "system");
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 12);
        assert!(body["generationConfig"].get("tools").is_none());

        let source = futures_util::stream::iter([
            Ok(b"data: {\"responseId\":\"res".to_vec()),
            Ok(b"ponse-1\",\"candidates\":[]}\r\n\r\n".to_vec()),
        ]);
        let mut state = SseState {
            source: Box::pin(source),
            buffer: Vec::new(),
            pending: VecDeque::new(),
            done: false,
            signal: None,
        };
        assert_eq!(
            state.next_event().await.unwrap().unwrap()["responseId"],
            "response-1"
        );
        assert!(state.next_event().await.unwrap().is_none());
    }
}
