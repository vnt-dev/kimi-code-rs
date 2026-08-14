//! Buffered cloud telemetry appender.
//!
//! Original: `packages/agent-core-v2/src/app/telemetry/cloudAppender.ts`.

use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::task::JoinHandle;

use crate::{
    _base::{
        di::{
            errors::DiError,
            instantiation::{ServicesAccessor, ServicesAccessorExt},
        },
        errors::unexpected_error::on_unexpected_error,
    },
    app::bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
    persistence::interface::storage::{FILE_SYSTEM_STORAGE_SERVICE_ID, FileSystemStorageService},
};

use super::{
    cloud_transport::{
        AccessTokenProvider, CloudContext, CloudEvent, CloudHttpClient, CloudNow, CloudPrimitive,
        CloudProperties, CloudSleep, CloudTransport, CloudTransportError, CloudTransportOptions,
        EnrichedCloudEvent, is_cloud_primitive,
    },
    contract::{
        TelemetryAppender, TelemetryAppenderResult, TelemetryContextPatch, TelemetryProperties,
    },
    core_version::resolve_core_version,
    privacy::clean_telemetry_string,
};

const DEFAULT_FLUSH_THRESHOLD: usize = 50;
const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_secs(30);

pub struct CloudAppenderOptions {
    pub storage: Arc<dyn FileSystemStorageService>,
    pub bootstrap: BootstrapServiceHandle,
    pub device_id: String,
    pub session_id: Option<String>,
    pub app_name: String,
    pub ui_mode: Option<String>,
    pub model: Option<String>,
    pub build_sha: Option<String>,
    pub terminal: Option<String>,
    pub locale: Option<String>,
    pub get_access_token: Option<AccessTokenProvider>,
    pub endpoint: Option<String>,
    pub flush_threshold: usize,
    pub flush_interval: Duration,
    pub http_client: Option<Arc<dyn CloudHttpClient>>,
    pub retry_backoffs: Option<Vec<Duration>>,
    pub request_timeout: Option<Duration>,
    pub sleep: Option<CloudSleep>,
    pub now: Option<CloudNow>,
}

impl CloudAppenderOptions {
    pub fn new(
        storage: Arc<dyn FileSystemStorageService>,
        bootstrap: BootstrapServiceHandle,
        device_id: impl Into<String>,
        app_name: impl Into<String>,
    ) -> Self {
        Self {
            storage,
            bootstrap,
            device_id: device_id.into(),
            session_id: None,
            app_name: app_name.into(),
            ui_mode: None,
            model: None,
            build_sha: None,
            terminal: None,
            locale: None,
            get_access_token: None,
            endpoint: None,
            flush_threshold: DEFAULT_FLUSH_THRESHOLD,
            flush_interval: DEFAULT_FLUSH_INTERVAL,
            http_client: None,
            retry_backoffs: None,
            request_timeout: None,
            sleep: None,
            now: None,
        }
    }
}

pub struct CloudAppenderHostOptions {
    pub device_id: String,
    pub app_name: String,
    pub ui_mode: Option<String>,
    pub model: Option<String>,
    pub build_sha: Option<String>,
    pub session_id: Option<String>,
    pub get_access_token: Option<AccessTokenProvider>,
}

// Original: createCloudAppender().
pub fn create_cloud_appender(
    accessor: &dyn ServicesAccessor,
    host: CloudAppenderHostOptions,
) -> Result<CloudAppender, DiError> {
    let storage = accessor.get(FILE_SYSTEM_STORAGE_SERVICE_ID)?;
    let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
    let mut options = CloudAppenderOptions::new(
        Arc::clone(&storage.0),
        (*bootstrap).clone(),
        host.device_id,
        host.app_name,
    );
    options.ui_mode = host.ui_mode;
    options.model = host.model;
    options.build_sha = host.build_sha;
    options.session_id = host.session_id;
    options.get_access_token = host.get_access_token;
    Ok(CloudAppender::new(options))
}

struct CloudAppenderState {
    device_id: String,
    session_id: Option<String>,
    context: CloudContext,
    buffer: Vec<EnrichedCloudEvent>,
}

struct CloudAppenderInner {
    state: Mutex<CloudAppenderState>,
    transport: CloudTransport,
    flush_threshold: usize,
    flush_interval: Duration,
    flush_task: Mutex<Option<JoinHandle<()>>>,
}

pub struct CloudAppender {
    inner: Arc<CloudAppenderInner>,
}

impl CloudAppender {
    // Original: CloudAppender.constructor().
    pub fn new(options: CloudAppenderOptions) -> Self {
        let context = build_context(&options);
        let mut transport_options =
            CloudTransportOptions::new(Arc::clone(&options.storage), options.device_id.clone());
        if let Some(endpoint) = options.endpoint {
            transport_options.endpoint = endpoint;
        }
        transport_options.get_access_token = options.get_access_token;
        if let Some(http_client) = options.http_client {
            transport_options.http_client = http_client;
        }
        if let Some(retry_backoffs) = options.retry_backoffs {
            transport_options.retry_backoffs = retry_backoffs;
        }
        if let Some(request_timeout) = options.request_timeout {
            transport_options.request_timeout = request_timeout;
        }
        if let Some(sleep) = options.sleep {
            transport_options.sleep = sleep;
        }
        if let Some(now) = options.now {
            transport_options.now = now;
        }
        Self {
            inner: Arc::new(CloudAppenderInner {
                state: Mutex::new(CloudAppenderState {
                    device_id: options.device_id,
                    session_id: options.session_id,
                    context,
                    buffer: Vec::new(),
                }),
                transport: CloudTransport::new(transport_options),
                flush_threshold: options.flush_threshold,
                flush_interval: options.flush_interval,
                flush_task: Mutex::new(None),
            }),
        }
    }

    // Original: CloudAppender.startPeriodicFlush().
    pub fn start_periodic_flush(&self) {
        let mut task = self.inner.flush_task.lock();
        if task.is_some() {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            on_unexpected_error(&std::io::Error::other(
                "cloud telemetry periodic flush requires a Tokio runtime",
            ));
            return;
        };
        let weak = Arc::downgrade(&self.inner);
        let interval = self.inner.flush_interval;
        *task = Some(runtime.spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                let _ = flush_inner(&inner).await;
            }
        }));
    }

    // Original: CloudAppender.stopPeriodicFlush().
    pub fn stop_periodic_flush(&self) {
        if let Some(task) = self.inner.flush_task.lock().take() {
            task.abort();
        }
    }

    // Original: CloudAppender.retryDiskEvents().
    pub async fn retry_disk_events(&self) -> Result<(), CloudTransportError> {
        self.inner.transport.retry_disk_events().await
    }
}

impl Drop for CloudAppender {
    fn drop(&mut self) {
        self.stop_periodic_flush();
    }
}

#[async_trait]
impl TelemetryAppender for CloudAppender {
    // Original: CloudAppender.track(). Threshold flush takes the buffer before
    // scheduling I/O, matching the synchronous prefix of the source async call.
    fn track(&self, event: &str, properties: Option<&TelemetryProperties>) {
        let properties = clean_properties(sanitize_properties(properties));
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            let mut state = self.inner.state.lock();
            let enriched = enriched_event(&state, event, properties);
            state.buffer.push(enriched);
            if state.buffer.len() >= self.inner.flush_threshold {
                on_unexpected_error(&std::io::Error::other(
                    "cloud telemetry threshold flush requires a Tokio runtime",
                ));
            }
            return;
        };
        let events = {
            let mut state = self.inner.state.lock();
            let enriched = enriched_event(&state, event, properties);
            state.buffer.push(enriched);
            (state.buffer.len() >= self.inner.flush_threshold)
                .then(|| std::mem::take(&mut state.buffer))
        };
        if let Some(events) = events {
            let inner = Arc::clone(&self.inner);
            runtime.spawn(async move {
                let _ = inner.transport.send(&events, None).await;
            });
        }
    }

    // Original: CloudAppender.setContext().
    fn set_context(&self, patch: &TelemetryContextPatch) {
        let mut state = self.inner.state.lock();
        if let Some(Value::String(device_id)) = patch.get("deviceId").and_then(Option::as_ref) {
            state.device_id.clone_from(device_id);
        }
        if let Some(Value::String(session_id)) = patch.get("sessionId").and_then(Option::as_ref) {
            state.session_id = Some(session_id.clone());
        }
        if let Some(Value::String(model)) = patch.get("model").and_then(Option::as_ref) {
            set_primitive(
                &mut state.context,
                "model",
                Some(Value::String(model.clone())),
            );
        }
    }

    // Original: CloudAppender.flush().
    async fn flush(&self) -> TelemetryAppenderResult {
        flush_inner(&self.inner)
            .await
            .map_err(|error| Box::new(error) as _)
    }

    // Original: CloudAppender.shutdown().
    async fn shutdown(&self) -> TelemetryAppenderResult {
        self.stop_periodic_flush();
        self.flush().await
    }
}

async fn flush_inner(inner: &CloudAppenderInner) -> Result<(), CloudTransportError> {
    let events = std::mem::take(&mut inner.state.lock().buffer);
    if events.is_empty() {
        return Ok(());
    }
    inner.transport.send(&events, None).await
}

fn enriched_event(
    state: &CloudAppenderState,
    event: &str,
    properties: CloudProperties,
) -> EnrichedCloudEvent {
    EnrichedCloudEvent {
        event: CloudEvent {
            event_id: uuid::Uuid::new_v4().simple().to_string(),
            device_id: Some(state.device_id.clone()),
            session_id: state.session_id.clone(),
            event: event.into(),
            timestamp: current_time_seconds(),
            properties,
        },
        context: state.context.clone(),
    }
}

// Original: sanitizeProperties().
fn sanitize_properties(properties: Option<&TelemetryProperties>) -> CloudProperties {
    properties.map_or_else(CloudProperties::new, |properties| {
        properties
            .iter()
            .filter_map(|(key, value)| {
                if is_cloud_primitive(value) {
                    Some((key.clone(), value.clone()))
                } else {
                    on_unexpected_error(&std::io::Error::other(format!(
                        "telemetry property \"{key}\" is not a primitive and was dropped"
                    )));
                    None
                }
            })
            .collect()
    })
}

fn clean_properties(mut properties: CloudProperties) -> CloudProperties {
    for value in properties.values_mut() {
        if let Some(Value::String(text)) = value {
            *text = clean_telemetry_string(text);
        }
    }
    properties
}

// Original: buildContext(). External field names and the legacy `node`
// runtime label remain unchanged for telemetry protocol compatibility.
fn build_context(options: &CloudAppenderOptions) -> CloudContext {
    let bootstrap = &options.bootstrap;
    let node_version = bootstrap
        .get_env("NODE_VERSION")
        .or_else(|| bootstrap.get_env("npm_config_node_version"))
        .unwrap_or("unknown");
    let mut context = CloudContext::from([
        (
            "app_name".into(),
            Some(Value::String(options.app_name.clone())),
        ),
        (
            "client_version".into(),
            Some(Value::String(bootstrap.client_version().into())),
        ),
        (
            "version".into(),
            Some(Value::String(bootstrap.client_version().into())),
        ),
        (
            "core_version".into(),
            Some(Value::String(resolve_core_version().into())),
        ),
        ("runtime".into(), Some(Value::String("node".into()))),
        (
            "platform".into(),
            Some(Value::String(bootstrap.platform().into())),
        ),
        ("arch".into(), Some(Value::String(bootstrap.arch().into()))),
        (
            "node_version".into(),
            Some(Value::String(node_version.into())),
        ),
        ("os_version".into(), Some(Value::String(os_version()))),
        (
            "ci".into(),
            Some(Value::Bool(bootstrap.get_env("CI").is_some())),
        ),
        (
            "locale".into(),
            Some(Value::String(
                options
                    .locale
                    .clone()
                    .or_else(|| bootstrap.get_env("LANG").map(str::to_owned))
                    .unwrap_or_default(),
            )),
        ),
        (
            "terminal".into(),
            Some(Value::String(
                options
                    .terminal
                    .clone()
                    .or_else(|| bootstrap.get_env("TERM_PROGRAM").map(str::to_owned))
                    .unwrap_or_default(),
            )),
        ),
        (
            "ui_mode".into(),
            Some(Value::String(
                options.ui_mode.clone().unwrap_or_else(|| "shell".into()),
            )),
        ),
    ]);
    set_primitive(
        &mut context,
        "model",
        options.model.clone().map(Value::String),
    );
    set_primitive(
        &mut context,
        "build_sha",
        options.build_sha.clone().map(Value::String),
    );
    context
}

// Original: setPrimitive().
fn set_primitive(target: &mut CloudContext, key: &str, value: CloudPrimitive) {
    let Some(value) = value else {
        return;
    };
    if value.as_str().is_some_and(str::is_empty) {
        return;
    }
    target.insert(key.into(), Some(value));
}

fn current_time_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64())
}

fn os_version() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|release| release.trim().to_owned())
        .unwrap_or_else(|_| std::env::consts::OS.into())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf};

    use indexmap::IndexMap;
    use serde_json::json;
    use tokio::sync::Notify;

    use crate::{
        app::bootstrap::{BootstrapOptions, BootstrapService, BootstrapServiceContract},
        persistence::backends::memory::in_memory_storage_service::InMemoryStorageService,
    };

    use super::*;

    #[derive(Default)]
    struct RecordingHttp {
        bodies: Mutex<Vec<Value>>,
        sent: Notify,
    }

    #[async_trait]
    impl CloudHttpClient for RecordingHttp {
        async fn post(
            &self,
            _url: &str,
            _headers: &IndexMap<String, String>,
            body: &str,
            _timeout: Duration,
            _signal: Option<&crate::_base::utils::abort::AbortSignal>,
        ) -> Result<u16, super::super::CloudHttpError> {
            self.bodies.lock().push(serde_json::from_str(body).unwrap());
            self.sent.notify_one();
            Ok(200)
        }
    }

    fn bootstrap() -> BootstrapServiceHandle {
        let service: Arc<dyn BootstrapServiceContract> =
            Arc::new(BootstrapService::new(BootstrapOptions {
                home_dir: PathBuf::from("/tmp/kimi"),
                config_path: PathBuf::from("/tmp/kimi/config.toml"),
                os_home_dir: PathBuf::from("/home/test"),
                platform: "linux".into(),
                arch: "x64".into(),
                cwd: PathBuf::from("/work"),
                env: HashMap::from([
                    ("CI".into(), "1".into()),
                    ("LANG".into(), "en_US.UTF-8".into()),
                    ("NODE_VERSION".into(), "22.0.0".into()),
                ]),
                client_version: "1.0.0".into(),
            }));
        BootstrapServiceHandle(service)
    }

    fn options(http: Arc<RecordingHttp>) -> CloudAppenderOptions {
        let storage: Arc<dyn FileSystemStorageService> =
            Arc::new(InMemoryStorageService::default());
        let mut options = CloudAppenderOptions::new(storage, bootstrap(), "dev", "test-app");
        options.http_client = Some(http);
        options.retry_backoffs = Some(Vec::new());
        options
    }

    #[tokio::test]
    async fn flushes_cleaned_flattened_events_and_applies_context_updates() {
        let http = Arc::new(RecordingHttp::default());
        let appender = CloudAppender::new(options(Arc::clone(&http)));
        appender.set_context(&TelemetryContextPatch::from([
            ("deviceId".into(), Some(Value::from("dev2"))),
            ("sessionId".into(), Some(Value::from("session"))),
            ("model".into(), Some(Value::from("kimi"))),
        ]));
        appender.track(
            "tool.call",
            Some(&TelemetryProperties::from([
                (
                    "message".into(),
                    Some(Value::from("see /home/alice/private.txt")),
                ),
                ("count".into(), Some(Value::from(2))),
                ("bad".into(), Some(json!({ "nested": true }))),
            ])),
        );
        appender.flush().await.unwrap();

        let bodies = http.bodies.lock();
        let body = &bodies[0];
        assert_eq!(body["user_id"], "kfc_device_id_dev");
        let event = &body["events"][0];
        assert_eq!(event["event"], "kfc_tool.call");
        assert_eq!(event["device_id"], "dev2");
        assert_eq!(event["session_id"], "session");
        assert_eq!(event["property_count"], 2);
        assert_eq!(event["property_message"], "see <REDACTED: user-file-path>");
        assert!(event.get("property_bad").is_none());
        assert_eq!(event["context_app_name"], "test-app");
        assert_eq!(event["context_client_version"], "1.0.0");
        assert_eq!(event["context_runtime"], "node");
        assert_eq!(event["context_node_version"], "22.0.0");
        assert_eq!(event["context_model"], "kimi");
        assert_eq!(event["context_ci"], true);
        assert_eq!(event["event_id"].as_str().unwrap().len(), 32);
        assert!(event["timestamp"].is_number());
    }

    #[tokio::test]
    async fn threshold_and_periodic_flushes_are_single_start_and_shutdown_safe() {
        let threshold_http = Arc::new(RecordingHttp::default());
        let mut threshold_options = options(Arc::clone(&threshold_http));
        threshold_options.flush_threshold = 2;
        let threshold = CloudAppender::new(threshold_options);
        threshold.track("one", None);
        threshold.track("two", None);
        tokio::time::timeout(Duration::from_secs(1), threshold_http.sent.notified())
            .await
            .unwrap();
        assert_eq!(
            threshold_http.bodies.lock()[0]["events"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        threshold.shutdown().await.unwrap();

        let periodic_http = Arc::new(RecordingHttp::default());
        let mut periodic_options = options(Arc::clone(&periodic_http));
        periodic_options.flush_interval = Duration::from_millis(1);
        let periodic = CloudAppender::new(periodic_options);
        periodic.track("periodic", None);
        periodic.start_periodic_flush();
        periodic.start_periodic_flush();
        tokio::time::timeout(Duration::from_secs(1), periodic_http.sent.notified())
            .await
            .unwrap();
        periodic.stop_periodic_flush();
        periodic.shutdown().await.unwrap();
        assert_eq!(periodic_http.bodies.lock().len(), 1);
    }
}
