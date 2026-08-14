//! Cloud telemetry wire values, payload transforms, and HTTP transport.
//!
//! Original: pure helpers and wire contracts from
//! `packages/agent-core-v2/src/app/telemetry/cloudTransport.ts`.

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use async_trait::async_trait;
use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{Map, Number, Value};

use crate::{
    _base::utils::{
        abort::{AbortError, AbortSignal},
        hash::encode_hex,
    },
    persistence::interface::storage::{
        FileSystemStorageService, StorageError, StorageWriteOptions,
    },
};

pub type CloudPrimitive = Option<Value>;
pub type CloudProperties = IndexMap<String, CloudPrimitive>;
pub type CloudContext = CloudProperties;

#[derive(Clone, Debug, PartialEq)]
pub struct CloudEvent {
    pub event_id: String,
    pub device_id: Option<String>,
    pub session_id: Option<String>,
    pub event: String,
    pub timestamp: f64,
    pub properties: CloudProperties,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnrichedCloudEvent {
    pub event: CloudEvent,
    pub context: CloudContext,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CloudPayload {
    pub user_id: String,
    pub events: Vec<CloudProperties>,
}

impl CloudPayload {
    pub fn to_json_value(&self) -> Value {
        Value::Object(Map::from_iter([
            ("user_id".into(), Value::String(self.user_id.clone())),
            (
                "events".into(),
                Value::Array(
                    self.events
                        .iter()
                        .map(cloud_properties_to_json_value)
                        .collect(),
                ),
            ),
        ]))
    }
}

pub const TELEMETRY_ENDPOINT: &str = "https://telemetry-logs.kimi.com/v1/event";
pub const SERVER_EVENT_PREFIX: &str = "kfc_";
pub const USER_ID_PREFIX: &str = "kfc_device_id_";
pub const DISK_EVENT_MAX_AGE_MS: f64 = 604_800_000.0;
pub const RETRY_BACKOFFS_MS: [u64; 3] = [1_000, 4_000, 16_000];

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 10_000;
const TELEMETRY_SCOPE: &str = "telemetry";
const FAILED_PREFIX: &str = "failed_";
const JSONL_SUFFIX: &str = ".jsonl";

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct CloudPayloadError {
    message: String,
}

impl CloudPayloadError {
    fn non_primitive(key: &str) -> Self {
        Self {
            message: format!("telemetry {key} must be primitive"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{0}")]
pub struct TransientCloudError(pub String);

#[derive(Debug, thiserror::Error)]
pub enum CloudTransportError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error(transparent)]
    Aborted(Arc<AbortError>),
}

#[derive(Debug, thiserror::Error)]
pub enum CloudHttpError {
    #[error(transparent)]
    Aborted(Arc<AbortError>),
    #[error("telemetry request timed out")]
    Timeout,
    #[error("{0}")]
    Request(String),
}

#[async_trait]
pub trait CloudHttpClient: Send + Sync {
    async fn post(
        &self,
        url: &str,
        headers: &IndexMap<String, String>,
        body: &str,
        timeout: Duration,
        signal: Option<&AbortSignal>,
    ) -> Result<u16, CloudHttpError>;
}

#[derive(Clone, Default)]
pub struct ReqwestCloudHttpClient {
    client: reqwest::Client,
}

#[async_trait]
impl CloudHttpClient for ReqwestCloudHttpClient {
    async fn post(
        &self,
        url: &str,
        headers: &IndexMap<String, String>,
        body: &str,
        timeout: Duration,
        signal: Option<&AbortSignal>,
    ) -> Result<u16, CloudHttpError> {
        let mut request = self.client.post(url).body(body.to_owned());
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let send = request.send();
        let response = if let Some(signal) = signal {
            tokio::select! {
                biased;
                reason = signal.cancelled() => return Err(CloudHttpError::Aborted(reason)),
                result = tokio::time::timeout(timeout, send) => {
                    result.map_err(|_| CloudHttpError::Timeout)?
                }
            }
        } else {
            tokio::time::timeout(timeout, send)
                .await
                .map_err(|_| CloudHttpError::Timeout)?
        }
        .map_err(|error| CloudHttpError::Request(error.to_string()))?;
        Ok(response.status().as_u16())
    }
}

pub type CloudFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
pub type AccessTokenProvider =
    Arc<dyn Fn() -> CloudFuture<Result<Option<String>, String>> + Send + Sync>;
pub type CloudSleep = Arc<
    dyn Fn(Duration, Option<AbortSignal>) -> CloudFuture<Result<(), CloudSleepError>> + Send + Sync,
>;
pub type CloudNow = Arc<dyn Fn() -> f64 + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum CloudSleepError {
    #[error(transparent)]
    Aborted(Arc<AbortError>),
    #[error("{0}")]
    Failed(String),
}

pub struct CloudTransportOptions {
    pub storage: Arc<dyn FileSystemStorageService>,
    pub device_id: String,
    pub endpoint: String,
    pub get_access_token: Option<AccessTokenProvider>,
    pub http_client: Arc<dyn CloudHttpClient>,
    pub retry_backoffs: Vec<Duration>,
    pub request_timeout: Duration,
    pub sleep: CloudSleep,
    pub now: CloudNow,
}

impl CloudTransportOptions {
    pub fn new(storage: Arc<dyn FileSystemStorageService>, device_id: impl Into<String>) -> Self {
        Self {
            storage,
            device_id: device_id.into(),
            endpoint: TELEMETRY_ENDPOINT.into(),
            get_access_token: None,
            http_client: Arc::new(ReqwestCloudHttpClient::default()),
            retry_backoffs: RETRY_BACKOFFS_MS
                .into_iter()
                .map(Duration::from_millis)
                .collect(),
            request_timeout: Duration::from_millis(DEFAULT_REQUEST_TIMEOUT_MS),
            sleep: Arc::new(|duration, signal| {
                Box::pin(async move {
                    abortable_sleep(duration, signal.as_ref())
                        .await
                        .map_err(CloudSleepError::Aborted)
                })
            }),
            now: Arc::new(current_time_millis),
        }
    }
}

pub struct CloudTransport {
    storage: Arc<dyn FileSystemStorageService>,
    device_id: String,
    endpoint: String,
    get_access_token: Option<AccessTokenProvider>,
    http_client: Arc<dyn CloudHttpClient>,
    retry_backoffs: Vec<Duration>,
    request_timeout: Duration,
    sleep: CloudSleep,
    now: CloudNow,
}

impl CloudTransport {
    // Original: CloudTransport.constructor().
    pub fn new(options: CloudTransportOptions) -> Self {
        Self {
            storage: options.storage,
            device_id: options.device_id,
            endpoint: options.endpoint,
            get_access_token: options.get_access_token,
            http_client: options.http_client,
            retry_backoffs: options.retry_backoffs,
            request_timeout: options.request_timeout,
            sleep: options.sleep,
            now: options.now,
        }
    }

    // Original: CloudTransport.send(). HTTP failures are intentionally
    // persisted and swallowed; storage and caller cancellation remain visible.
    pub async fn send(
        &self,
        events: &[EnrichedCloudEvent],
        signal: Option<&AbortSignal>,
    ) -> Result<(), CloudTransportError> {
        if events.is_empty() {
            return Ok(());
        }
        if let Some(reason) = aborted_reason(signal) {
            self.save_to_disk(events).await?;
            return Err(CloudTransportError::Aborted(reason));
        }
        let payload = match build_payload(events, &self.device_id) {
            Ok(payload) => payload,
            Err(_) => return Ok(()),
        };

        for attempt in 0..=self.retry_backoffs.len() {
            match self.send_http(&payload, signal).await {
                Ok(()) => return Ok(()),
                Err(SendHttpError::Aborted(reason)) => {
                    self.save_to_disk(events).await?;
                    return Err(CloudTransportError::Aborted(reason));
                }
                Err(SendHttpError::Permanent) => break,
                Err(SendHttpError::Transient) => {
                    let Some(backoff) = self.retry_backoffs.get(attempt).copied() else {
                        break;
                    };
                    match (self.sleep)(backoff, signal.cloned()).await {
                        Ok(()) => {}
                        Err(CloudSleepError::Aborted(reason)) => {
                            self.save_to_disk(events).await?;
                            return Err(CloudTransportError::Aborted(reason));
                        }
                        Err(CloudSleepError::Failed(_)) => break,
                    }
                }
            }
        }
        self.save_to_disk(events).await
    }

    // Original: CloudTransport.saveToDisk().
    pub async fn save_to_disk(
        &self,
        events: &[EnrichedCloudEvent],
    ) -> Result<(), CloudTransportError> {
        if events.is_empty() {
            return Ok(());
        }
        let key = format!(
            "{FAILED_PREFIX}{}_{}{JSONL_SUFFIX}",
            format_js_number((self.now)()),
            random_hex_12()
        );
        let mut text = String::new();
        for event in events {
            text.push_str(&serde_json::to_string(&enriched_event_to_json_value(
                event,
            ))?);
            text.push('\n');
        }
        self.storage
            .write(
                TELEMETRY_SCOPE,
                &key,
                text.as_bytes(),
                StorageWriteOptions::default(),
            )
            .await?;
        Ok(())
    }

    // Original: CloudTransport.retryDiskEvents(). Files are processed in the
    // storage listing order and one transient failure does not block later files.
    pub async fn retry_disk_events(&self) -> Result<(), CloudTransportError> {
        let keys = self
            .storage
            .list(TELEMETRY_SCOPE, Some(FAILED_PREFIX))
            .await?;
        let now = (self.now)();
        for key in keys {
            if !key.starts_with(FAILED_PREFIX) || !key.ends_with(JSONL_SUFFIX) {
                continue;
            }
            let created_at = parse_failed_timestamp(&key);
            if created_at.is_none_or(|created_at| now - created_at > DISK_EVENT_MAX_AGE_MS) {
                let _ = self.storage.delete(TELEMETRY_SCOPE, &key).await;
                continue;
            }

            let events = match self.read_jsonl(&key).await {
                Ok(events) => events,
                Err(ReadJsonlError::Invalid) => {
                    let _ = self.storage.delete(TELEMETRY_SCOPE, &key).await;
                    continue;
                }
                Err(ReadJsonlError::Storage) => continue,
            };
            let payload = match build_payload(&events, &self.device_id) {
                Ok(payload) => payload,
                Err(_) => {
                    let _ = self.storage.delete(TELEMETRY_SCOPE, &key).await;
                    continue;
                }
            };
            match self.send_http(&payload, None).await {
                Ok(()) => self.storage.delete(TELEMETRY_SCOPE, &key).await?,
                Err(
                    SendHttpError::Transient | SendHttpError::Permanent | SendHttpError::Aborted(_),
                ) => {}
            }
        }
        Ok(())
    }

    // Original: CloudTransport.readJsonl().
    async fn read_jsonl(&self, key: &str) -> Result<Vec<EnrichedCloudEvent>, ReadJsonlError> {
        let bytes = self
            .storage
            .read(TELEMETRY_SCOPE, key)
            .await
            .map_err(|_| ReadJsonlError::Storage)?;
        let Some(bytes) = bytes else {
            return Ok(Vec::new());
        };
        let text = String::from_utf8(bytes).map_err(|_| ReadJsonlError::Invalid)?;
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| {
                serde_json::from_str::<DiskEnrichedCloudEvent>(line)
                    .map(Into::into)
                    .map_err(|_| ReadJsonlError::Invalid)
            })
            .collect()
    }

    // Original: CloudTransport.sendHttp().
    async fn send_http(
        &self,
        payload: &CloudPayload,
        signal: Option<&AbortSignal>,
    ) -> Result<(), SendHttpError> {
        let token = match &self.get_access_token {
            Some(provider) => provider().await.map_err(|_| SendHttpError::Permanent)?,
            None => None,
        };
        let mut headers = IndexMap::from([("Content-Type".into(), "application/json".into())]);
        if let Some(token) = token.filter(|token| !token.is_empty()) {
            headers.insert("Authorization".into(), format!("Bearer {token}"));
        }
        let body = serde_json::to_string(&payload.to_json_value())
            .map_err(|_| SendHttpError::Permanent)?;
        let status = self.post(&headers, &body, signal).await?;
        if status == 401 && headers.shift_remove("Authorization").is_some() {
            let retry = self.post(&headers, &body, signal).await?;
            return handle_status(retry);
        }
        handle_status(status)
    }

    // Original: CloudTransport.post().
    async fn post(
        &self,
        headers: &IndexMap<String, String>,
        body: &str,
        signal: Option<&AbortSignal>,
    ) -> Result<u16, SendHttpError> {
        self.http_client
            .post(&self.endpoint, headers, body, self.request_timeout, signal)
            .await
            .map_err(|error| match error {
                CloudHttpError::Aborted(reason) => SendHttpError::Aborted(reason),
                CloudHttpError::Timeout | CloudHttpError::Request(_) => SendHttpError::Transient,
            })
    }
}

#[derive(Debug)]
enum SendHttpError {
    Aborted(Arc<AbortError>),
    Transient,
    Permanent,
}

enum ReadJsonlError {
    Invalid,
    Storage,
}

#[derive(Deserialize)]
struct DiskEnrichedCloudEvent {
    event_id: String,
    device_id: Option<String>,
    session_id: Option<String>,
    event: String,
    timestamp: f64,
    properties: IndexMap<String, Value>,
    context: IndexMap<String, Value>,
}

impl From<DiskEnrichedCloudEvent> for EnrichedCloudEvent {
    fn from(event: DiskEnrichedCloudEvent) -> Self {
        Self {
            event: CloudEvent {
                event_id: event.event_id,
                device_id: event.device_id,
                session_id: event.session_id,
                event: event.event,
                timestamp: event.timestamp,
                properties: event
                    .properties
                    .into_iter()
                    .map(|(key, value)| (key, Some(value)))
                    .collect(),
            },
            context: event
                .context
                .into_iter()
                .map(|(key, value)| (key, Some(value)))
                .collect(),
        }
    }
}

fn enriched_event_to_json_value(event: &EnrichedCloudEvent) -> Value {
    Value::Object(Map::from_iter([
        (
            "event_id".into(),
            Value::String(event.event.event_id.clone()),
        ),
        (
            "device_id".into(),
            event
                .event
                .device_id
                .clone()
                .map_or(Value::Null, Value::String),
        ),
        (
            "session_id".into(),
            event
                .event
                .session_id
                .clone()
                .map_or(Value::Null, Value::String),
        ),
        ("event".into(), Value::String(event.event.event.clone())),
        (
            "timestamp".into(),
            Number::from_f64(event.event.timestamp)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        ),
        (
            "properties".into(),
            cloud_properties_to_json_value(&event.event.properties),
        ),
        (
            "context".into(),
            cloud_properties_to_json_value(&event.context),
        ),
    ]))
}

// Original: parseFailedTimestamp().
fn parse_failed_timestamp(key: &str) -> Option<f64> {
    let rest = key.strip_prefix(FAILED_PREFIX)?;
    let (raw, _) = rest.split_once('_')?;
    raw.parse::<f64>()
        .ok()
        .filter(|timestamp| timestamp.is_finite())
}

// Original: handleStatus().
fn handle_status(status: u16) -> Result<(), SendHttpError> {
    if status >= 500 || status == 429 {
        Err(SendHttpError::Transient)
    } else {
        Ok(())
    }
}

async fn abortable_sleep(
    duration: Duration,
    signal: Option<&AbortSignal>,
) -> Result<(), Arc<AbortError>> {
    if let Some(signal) = signal {
        tokio::select! {
            biased;
            reason = signal.cancelled() => Err(reason),
            () = tokio::time::sleep(duration) => Ok(()),
        }
    } else {
        tokio::time::sleep(duration).await;
        Ok(())
    }
}

fn aborted_reason(signal: Option<&AbortSignal>) -> Option<Arc<AbortError>> {
    signal.and_then(AbortSignal::reason)
}

fn current_time_millis() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_millis() as f64)
}

fn format_js_number(number: f64) -> String {
    if number.fract() == 0.0 {
        format!("{number:.0}")
    } else {
        number.to_string()
    }
}

fn random_hex_12() -> String {
    let bytes = uuid::Uuid::new_v4();
    encode_hex(&bytes.as_bytes()[..6])
}

// Original: buildUserId().
pub fn build_user_id(device_id: &str) -> String {
    format!("{USER_ID_PREFIX}{device_id}")
}

// Original: buildPayload().
pub fn build_payload(
    events: &[EnrichedCloudEvent],
    device_id: &str,
) -> Result<CloudPayload, CloudPayloadError> {
    Ok(CloudPayload {
        user_id: build_user_id(device_id),
        events: events
            .iter()
            .map(|event| flatten_event(&apply_server_prefix(event)))
            .collect::<Result<_, _>>()?,
    })
}

// Original: applyServerPrefix().
pub fn apply_server_prefix(event: &EnrichedCloudEvent) -> EnrichedCloudEvent {
    if event.event.event.is_empty() || event.event.event.starts_with(SERVER_EVENT_PREFIX) {
        return event.clone();
    }
    let mut prefixed = event.clone();
    prefixed.event.event = format!("{SERVER_EVENT_PREFIX}{}", event.event.event);
    prefixed
}

// Original: flattenEvent().
pub fn flatten_event(event: &EnrichedCloudEvent) -> Result<CloudProperties, CloudPayloadError> {
    let mut output = CloudProperties::new();
    insert_primitive(
        &mut output,
        "event_id",
        Some(Value::String(event.event.event_id.clone())),
    )?;
    insert_primitive(
        &mut output,
        "device_id",
        Some(
            event
                .event
                .device_id
                .clone()
                .map_or(Value::Null, Value::String),
        ),
    )?;
    insert_primitive(
        &mut output,
        "session_id",
        Some(
            event
                .event
                .session_id
                .clone()
                .map_or(Value::Null, Value::String),
        ),
    )?;
    insert_primitive(
        &mut output,
        "event",
        Some(Value::String(event.event.event.clone())),
    )?;
    let timestamp = Number::from_f64(event.event.timestamp)
        .map(Value::Number)
        .ok_or_else(|| CloudPayloadError::non_primitive("timestamp"))?;
    insert_primitive(&mut output, "timestamp", Some(timestamp))?;
    flatten_nested(&mut output, "property", &event.event.properties)?;
    flatten_nested(&mut output, "context", &event.context)?;
    Ok(output)
}

// Original: isCloudPrimitive(). `None` is JavaScript `undefined`.
pub fn is_cloud_primitive(value: &CloudPrimitive) -> bool {
    match value {
        None | Some(Value::Null | Value::Bool(_) | Value::String(_)) => true,
        Some(Value::Number(number)) => number
            .as_f64()
            .is_some_and(|number| number.is_finite() && number.abs() <= MAX_SAFE_INTEGER),
        Some(Value::Array(_) | Value::Object(_)) => false,
    }
}

fn flatten_nested(
    target: &mut CloudProperties,
    prefix: &str,
    values: &CloudProperties,
) -> Result<(), CloudPayloadError> {
    for (key, value) in values {
        if !is_cloud_primitive(value) {
            return Err(CloudPayloadError::non_primitive(&format!("{prefix}.{key}")));
        }
        target.insert(format!("{prefix}_{key}"), value.clone());
    }
    Ok(())
}

fn insert_primitive(
    target: &mut CloudProperties,
    key: &str,
    value: CloudPrimitive,
) -> Result<(), CloudPayloadError> {
    if !is_cloud_primitive(&value) {
        return Err(CloudPayloadError::non_primitive(key));
    }
    target.insert(key.into(), value);
    Ok(())
}

pub(crate) fn cloud_properties_to_json_value(properties: &CloudProperties) -> Value {
    Value::Object(Map::from_iter(properties.iter().filter_map(
        |(key, value)| value.clone().map(|value| (key.clone(), value)),
    )))
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::collections::VecDeque;

    use serde_json::json;

    use crate::{
        _base::utils::abort::{AbortController, abort_error},
        persistence::{
            backends::memory::in_memory_storage_service::InMemoryStorageService,
            interface::storage::FileSystemStorageService,
        },
    };

    use super::*;

    #[derive(Clone)]
    struct CapturedRequest {
        headers: IndexMap<String, String>,
        body: String,
    }

    #[derive(Default)]
    struct MockHttpClient {
        responses: Mutex<VecDeque<Result<u16, CloudHttpError>>>,
        requests: Mutex<Vec<CapturedRequest>>,
    }

    impl MockHttpClient {
        fn with_statuses(statuses: impl IntoIterator<Item = u16>) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(statuses.into_iter().map(Ok).collect()),
                requests: Mutex::new(Vec::new()),
            })
        }

        fn push_status(&self, status: u16) {
            self.responses.lock().push_back(Ok(status));
        }
    }

    #[async_trait]
    impl CloudHttpClient for MockHttpClient {
        async fn post(
            &self,
            _url: &str,
            headers: &IndexMap<String, String>,
            body: &str,
            _timeout: Duration,
            _signal: Option<&AbortSignal>,
        ) -> Result<u16, CloudHttpError> {
            self.requests.lock().push(CapturedRequest {
                headers: headers.clone(),
                body: body.into(),
            });
            self.responses.lock().pop_front().unwrap_or(Ok(200))
        }
    }

    fn event(name: &str) -> EnrichedCloudEvent {
        EnrichedCloudEvent {
            event: CloudEvent {
                event_id: "event-1".into(),
                device_id: Some("dev".into()),
                session_id: None,
                event: name.into(),
                timestamp: 123.5,
                properties: CloudProperties::from([
                    ("name".into(), Some(Value::from("bash"))),
                    ("missing".into(), None),
                ]),
            },
            context: CloudContext::from([("app_name".into(), Some(Value::from("test")))]),
        }
    }

    #[test]
    fn builds_prefixed_flattened_payload_and_omits_undefined_from_json() {
        let payload = build_payload(&[event("tool.call")], "dev").unwrap();
        assert_eq!(payload.user_id, "kfc_device_id_dev");
        assert_eq!(
            payload.events[0]["event"],
            Some(Value::from("kfc_tool.call"))
        );
        assert_eq!(
            payload.events[0]["property_name"],
            Some(Value::from("bash"))
        );
        assert_eq!(payload.events[0]["property_missing"], None);
        assert_eq!(
            payload.events[0]["context_app_name"],
            Some(Value::from("test"))
        );
        assert_eq!(payload.events[0]["session_id"], Some(Value::Null));
        assert_eq!(
            payload.to_json_value(),
            json!({
                "user_id": "kfc_device_id_dev",
                "events": [{
                    "event_id": "event-1",
                    "device_id": "dev",
                    "session_id": null,
                    "event": "kfc_tool.call",
                    "timestamp": 123.5,
                    "property_name": "bash",
                    "context_app_name": "test"
                }]
            })
        );
    }

    #[test]
    fn preserves_existing_prefix_and_rejects_non_primitives_or_unsafe_numbers() {
        let prefixed = event("kfc_exit");
        assert_eq!(apply_server_prefix(&prefixed), prefixed);

        let mut invalid = event("evt");
        invalid
            .event
            .properties
            .insert("nested".into(), Some(json!({ "bad": true })));
        assert_eq!(
            build_payload(&[invalid], "dev").unwrap_err().to_string(),
            "telemetry property.nested must be primitive"
        );
        assert!(!is_cloud_primitive(&Some(Value::from(
            9_007_199_254_740_992_u64
        ))));
    }

    fn transport(http: Arc<MockHttpClient>) -> (CloudTransport, Arc<InMemoryStorageService>) {
        let storage = Arc::new(InMemoryStorageService::default());
        let erased_storage: Arc<dyn FileSystemStorageService> = storage.clone();
        let mut options = CloudTransportOptions::new(erased_storage, "dev");
        options.http_client = http;
        options.retry_backoffs = vec![Duration::ZERO; 3];
        options.sleep = Arc::new(|_, _| Box::pin(async { Ok(()) }));
        options.now = Arc::new(|| 1_000.0);
        (CloudTransport::new(options), storage)
    }

    #[tokio::test]
    async fn retries_persists_and_replays_failed_events() {
        let http = MockHttpClient::with_statuses([500, 500, 500, 500]);
        let (transport, storage) = transport(Arc::clone(&http));

        transport.send(&[event("evt")], None).await.unwrap();
        assert_eq!(http.requests.lock().len(), 4);
        let keys = storage
            .list(TELEMETRY_SCOPE, Some(FAILED_PREFIX))
            .await
            .unwrap();
        assert_eq!(keys.len(), 1);
        assert!(keys[0].starts_with("failed_1000_"));
        let disk = storage
            .read(TELEMETRY_SCOPE, &keys[0])
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8(disk).unwrap().ends_with('\n'));

        http.push_status(200);
        transport.retry_disk_events().await.unwrap();
        assert!(
            storage
                .list(TELEMETRY_SCOPE, Some(FAILED_PREFIX))
                .await
                .unwrap()
                .is_empty()
        );
        let replay = http.requests.lock().last().unwrap().clone();
        assert!(replay.body.contains("kfc_evt"));
    }

    #[tokio::test]
    async fn retries_unauthorized_without_auth_and_persists_on_caller_abort() {
        let http = MockHttpClient::with_statuses([401, 200]);
        let storage = Arc::new(InMemoryStorageService::default());
        let erased_storage: Arc<dyn FileSystemStorageService> = storage.clone();
        let mut options = CloudTransportOptions::new(erased_storage, "dev");
        options.http_client = http.clone();
        options.get_access_token = Some(Arc::new(|| Box::pin(async { Ok(Some("token".into())) })));
        options.now = Arc::new(|| 2_000.0);
        let transport = CloudTransport::new(options);

        transport.send(&[event("evt")], None).await.unwrap();
        {
            let requests = http.requests.lock();
            assert_eq!(requests[0].headers["Authorization"], "Bearer token");
            assert!(!requests[1].headers.contains_key("Authorization"));
        }

        let abort = AbortController::new();
        abort.abort(Some(abort_error(Some("stop"))));
        let signal = abort.signal();
        let error = transport
            .send(&[event("cancelled")], Some(&signal))
            .await
            .unwrap_err();
        assert!(matches!(error, CloudTransportError::Aborted(_)));
        assert_eq!(
            storage
                .list(TELEMETRY_SCOPE, Some(FAILED_PREFIX))
                .await
                .unwrap()
                .len(),
            1
        );
    }
}
