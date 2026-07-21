use std::{io::Write, path::PathBuf, sync::Mutex};

use async_trait::async_trait;
use indexmap::IndexMap;
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use serde_json::Value;

use super::{
    provider::{
        Catalog, CatalogFetchError, CatalogProviderEntry, CustomRegistryLimit,
        CustomRegistryModalities, CustomRegistryModelEntry, CustomRegistryProviderEntry,
        CustomRegistrySource, ProviderCatalogRuntime, ProviderConfig, ProviderConfigPatch,
        ProviderError, ProviderRegistryRuntime, ProviderRuntime, RegistryFetchError,
    },
    provider_config::ProviderConfigStore,
};

pub struct ProviderCommandRuntime {
    store: ProviderConfigStore,
    client: reqwest::Client,
    user_agent: String,
    stdout: Mutex<Box<dyn Write + Send>>,
    stderr: Mutex<Box<dyn Write + Send>>,
}

impl ProviderCommandRuntime {
    pub fn new(config_path: impl Into<PathBuf>, user_agent: impl Into<String>) -> Self {
        Self::with_io(
            config_path,
            user_agent,
            Box::new(std::io::stdout()),
            Box::new(std::io::stderr()),
        )
    }

    pub fn with_io(
        config_path: impl Into<PathBuf>,
        user_agent: impl Into<String>,
        stdout: Box<dyn Write + Send>,
        stderr: Box<dyn Write + Send>,
    ) -> Self {
        Self {
            store: ProviderConfigStore::new(config_path),
            client: reqwest::Client::new(),
            user_agent: user_agent.into(),
            stdout: Mutex::new(stdout),
            stderr: Mutex::new(stderr),
        }
    }
}

#[async_trait]
impl ProviderRuntime for ProviderCommandRuntime {
    async fn ensure_config_file(&self) -> Result<(), ProviderError> {
        self.store.ensure_config_file().await
    }

    async fn get_config(&self) -> Result<ProviderConfig, ProviderError> {
        self.store.get_config().await
    }

    async fn remove_provider(&self, provider_id: &str) -> Result<ProviderConfig, ProviderError> {
        self.store.remove_provider(provider_id).await
    }

    async fn set_config(
        &self,
        patch: &ProviderConfigPatch,
    ) -> Result<ProviderConfig, ProviderError> {
        self.store.set_config(patch).await
    }

    fn write_stdout(&self, text: &str) {
        if let Ok(mut stdout) = self.stdout.lock() {
            let _ = stdout.write_all(text.as_bytes());
        }
    }

    fn write_stderr(&self, text: &str) {
        if let Ok(mut stderr) = self.stderr.lock() {
            let _ = stderr.write_all(text.as_bytes());
        }
    }
}

#[async_trait]
impl ProviderRegistryRuntime for ProviderCommandRuntime {
    // Original:
    //   packages/oauth/src/custom-registry.ts
    //   fetchCustomRegistry()
    async fn fetch_custom_registry(
        &self,
        source: &CustomRegistrySource,
    ) -> Result<Vec<CustomRegistryProviderEntry>, RegistryFetchError> {
        let mut request = self
            .client
            .get(&source.url)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, &self.user_agent);
        if !source.api_key.is_empty() {
            request = request.header(AUTHORIZATION, format!("Bearer {}", source.api_key));
        }
        let response = request
            .send()
            .await
            .map_err(|error| RegistryFetchError::new(error, None))?;
        let status = response.status();
        if !status.is_success() {
            let fallback = format!(
                "Failed to fetch custom registry at {} (HTTP {}).",
                source.url,
                status.as_u16()
            );
            let message = match response.json::<Value>().await {
                Ok(value) => extract_api_error_message(&value).unwrap_or(fallback),
                Err(_) => fallback,
            };
            return Err(RegistryFetchError::new(
                RuntimeMessage(message),
                Some(status.as_u16()),
            ));
        }
        let payload = response
            .json::<Value>()
            .await
            .map_err(|error| RegistryFetchError::new(error, None))?;
        let object = payload.as_object().ok_or_else(|| {
            RegistryFetchError::new(
                RuntimeMessage(format!(
                    "Unexpected custom registry response at {}: expected a JSON object keyed by provider id.",
                    source.url
                )),
                None,
            )
        })?;
        let mut entries = Vec::new();
        for (key, value) in object {
            if let Some(entry) = parse_custom_registry_provider(value) {
                entries.push(entry);
            } else {
                self.write_stderr(&format!(
                    "[custom-registry] Skipping invalid entry \"{key}\" at {}: missing required fields or unsupported type (id, name, api, type, models).\n",
                    source.url
                ));
            }
        }
        Ok(entries)
    }
}

#[async_trait]
impl ProviderCatalogRuntime for ProviderCommandRuntime {
    // Original:
    //   packages/node-sdk/src/catalog.ts
    //   fetchCatalog()
    async fn fetch_catalog(&self, url: &str) -> Result<Catalog, CatalogFetchError> {
        let response = self
            .client
            .get(url)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, &self.user_agent)
            .send()
            .await
            .map_err(|error| CatalogFetchError::new(error, None))?;
        let status = response.status();
        if !status.is_success() {
            return Err(CatalogFetchError::new(
                RuntimeMessage(format!(
                    "Failed to fetch catalog (HTTP {}).",
                    status.as_u16()
                )),
                Some(status.as_u16()),
            ));
        }
        let payload = response
            .json::<Value>()
            .await
            .map_err(|error| CatalogFetchError::new(error, None))?;
        let object = payload.as_object().ok_or_else(|| {
            CatalogFetchError::new(
                RuntimeMessage(format!("Unexpected catalog response from {url}.")),
                None,
            )
        })?;
        object
            .iter()
            .map(|(id, entry)| {
                serde_json::from_value::<CatalogProviderEntry>(entry.clone())
                    .map(|entry| (id.clone(), entry))
                    .map_err(|error| CatalogFetchError::new(error, None))
            })
            .collect::<Result<IndexMap<_, _>, _>>()
    }
}

#[derive(Debug)]
struct RuntimeMessage(String);

impl std::fmt::Display for RuntimeMessage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RuntimeMessage {}

fn parse_custom_registry_provider(value: &Value) -> Option<CustomRegistryProviderEntry> {
    let object = value.as_object()?;
    let id = non_empty_string(object.get("id")?)?;
    let name = non_empty_string(object.get("name")?)?;
    let api = non_empty_string(object.get("api")?)?;
    let provider_type = non_empty_string(object.get("type")?)?;
    if !matches!(
        provider_type,
        "anthropic" | "openai" | "openai_responses" | "kimi"
    ) {
        return None;
    }
    let raw_models = object.get("models")?.as_object()?;
    let models = raw_models
        .iter()
        .filter_map(|(key, value)| {
            parse_custom_registry_model(value).map(|model| (key.clone(), model))
        })
        .collect();
    Some(CustomRegistryProviderEntry {
        id: id.to_owned(),
        name: name.to_owned(),
        api: api.to_owned(),
        provider_type: provider_type.to_owned(),
        env: string_array(object.get("env")),
        models,
    })
}

fn parse_custom_registry_model(value: &Value) -> Option<CustomRegistryModelEntry> {
    let object = value.as_object()?;
    let id = non_empty_string(object.get("id")?)?.to_owned();
    let name = object
        .get("name")
        .and_then(non_empty_string)
        .map(str::to_owned);
    let limit = object
        .get("limit")
        .and_then(Value::as_object)
        .and_then(|limit| {
            let context = positive_floor(limit.get("context"));
            let output = positive_floor(limit.get("output"));
            (context.is_some() || output.is_some())
                .then_some(CustomRegistryLimit { context, output })
        });
    let modalities = object
        .get("modalities")
        .and_then(Value::as_object)
        .and_then(|modalities| {
            let input = string_array(modalities.get("input"));
            let output = string_array(modalities.get("output"));
            (input.is_some() || output.is_some())
                .then_some(CustomRegistryModalities { input, output })
        });
    Some(CustomRegistryModelEntry {
        id,
        name,
        limit,
        tool_call: object.get("tool_call").and_then(Value::as_bool),
        reasoning: object.get("reasoning").and_then(Value::as_bool),
        modalities,
        support_efforts: string_array(object.get("support_efforts")),
        default_effort: object
            .get("default_effort")
            .and_then(non_empty_string)
            .map(str::to_owned),
    })
}

fn positive_floor(value: Option<&Value>) -> Option<u64> {
    let value = value?.as_f64()?;
    (value.is_finite() && value > 0.0 && value.floor() <= u64::MAX as f64)
        .then_some(value.floor() as u64)
}

fn string_array(value: Option<&Value>) -> Option<Vec<String>> {
    let values = value?.as_array()?;
    values
        .iter()
        .map(|value| value.as_str().map(str::to_owned))
        .collect()
}

fn non_empty_string(value: &Value) -> Option<&str> {
    value.as_str().filter(|value| !value.is_empty())
}

fn extract_api_error_message(value: &Value) -> Option<String> {
    if let Some(array) = value.as_array() {
        return array.iter().find_map(extract_api_error_message);
    }
    let object = value.as_object()?;
    for key in ["error_description", "message", "detail"] {
        if let Some(message) = object.get(key).and_then(trimmed_string) {
            return Some(message.to_owned());
        }
    }
    if let Some(message) = object.get("error").and_then(trimmed_string) {
        return Some(message.to_owned());
    }
    if let Some(error) = object.get("error").and_then(Value::as_object) {
        for key in ["message", "error_description", "detail", "code", "type"] {
            if let Some(message) = error.get(key).and_then(trimmed_string) {
                return Some(message.to_owned());
            }
        }
    }
    object
        .get("errors")
        .and_then(Value::as_array)
        .and_then(|errors| errors.iter().find_map(extract_api_error_message))
}

fn trimmed_string(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
    };

    use super::*;

    fn serve_once(
        status: &str,
        body: &str,
    ) -> (String, Arc<Mutex<String>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind server");
        let address = listener.local_addr().expect("server address");
        let request_capture = Arc::new(Mutex::new(String::new()));
        let capture = Arc::clone(&request_capture);
        let status = status.to_owned();
        let body = body.to_owned();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = [0_u8; 8192];
            let read = stream.read(&mut buffer).expect("read request");
            *capture.lock().expect("capture") = String::from_utf8_lossy(&buffer[..read]).into();
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("write response");
        });
        (format!("http://{address}"), request_capture, handle)
    }

    fn runtime() -> ProviderCommandRuntime {
        runtime_with_stderr(Box::new(Vec::<u8>::new()))
    }

    fn runtime_with_stderr(stderr: Box<dyn Write + Send>) -> ProviderCommandRuntime {
        ProviderCommandRuntime::with_io(
            std::env::temp_dir().join("unused-provider-runtime-config.toml"),
            "kimi-code-cli/test",
            Box::new(Vec::<u8>::new()),
            stderr,
        )
    }

    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("shared writer")
                .extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn registry_fetch_sends_identity_and_auth_and_skips_invalid_entries() {
        let body = r#"{
            "valid": {"id":"valid","name":"Valid","api":"https://api.test","type":"anthropic","models":{
                "ok":{"id":"ok","limit":{"context":1234.9},"tool_call":true},
                "bad":{"name":"missing id"}
            }},
            "unknown": {"id":"unknown","name":"Unknown","api":"https://api.test","type":"future","models":{}}
        }"#;
        let (url, request, server) = serve_once("200 OK", body);
        let source = CustomRegistrySource {
            kind: "apiJson".to_owned(),
            url,
            api_key: "secret".to_owned(),
        };

        let stderr = Arc::new(Mutex::new(Vec::new()));
        let entries = runtime_with_stderr(Box::new(SharedWriter(Arc::clone(&stderr))))
            .fetch_custom_registry(&source)
            .await
            .expect("registry");
        server.join().expect("server");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].models.len(), 1);
        assert_eq!(
            entries[0].models["ok"]
                .limit
                .as_ref()
                .expect("limit")
                .context,
            Some(1234)
        );
        let request = request.lock().expect("request").to_ascii_lowercase();
        assert!(request.contains("user-agent: kimi-code-cli/test"));
        assert!(request.contains("authorization: bearer secret"));
        assert!(request.contains("accept: application/json"));
        let warning = String::from_utf8(stderr.lock().expect("stderr").clone()).expect("UTF-8");
        assert!(warning.contains("Skipping invalid entry \"unknown\""));
    }

    #[tokio::test]
    async fn registry_fetch_extracts_nested_api_error_message() {
        let (url, _, server) = serve_once(
            "401 Unauthorized",
            r#"{"error":{"message":"  invalid token  "}}"#,
        );
        let source = CustomRegistrySource {
            kind: "apiJson".to_owned(),
            url,
            api_key: "bad".to_owned(),
        };

        let error = runtime()
            .fetch_custom_registry(&source)
            .await
            .expect_err("HTTP error");
        server.join().expect("server");

        assert_eq!(error.status, Some(401));
        assert_eq!(error.to_string(), "invalid token");
    }

    #[tokio::test]
    async fn catalog_fetch_preserves_provider_and_model_source_order() {
        let body = r#"{
            "second":{"id":"second","name":"Second","npm":"@ai-sdk/openai","models":{"b":{"id":"b"},"a":{"id":"a"}}},
            "first":{"id":"first","name":"First","models":{}}
        }"#;
        let (url, request, server) = serve_once("200 OK", body);

        let catalog = runtime().fetch_catalog(&url).await.expect("catalog");
        server.join().expect("server");

        assert_eq!(
            catalog.keys().map(String::as_str).collect::<Vec<_>>(),
            ["second", "first"]
        );
        assert_eq!(
            catalog["second"]
                .models
                .as_ref()
                .expect("models")
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["b", "a"]
        );
        assert!(
            request
                .lock()
                .expect("request")
                .to_ascii_lowercase()
                .contains("user-agent: kimi-code-cli/test")
        );
    }

    #[test]
    fn api_error_extraction_matches_direct_nested_array_precedence() {
        assert_eq!(
            extract_api_error_message(&serde_json::json!({
                "errors": [{ "detail": "first" }, { "message": "second" }]
            }))
            .as_deref(),
            Some("first")
        );
        assert_eq!(
            extract_api_error_message(&serde_json::json!({ "message": " direct " })).as_deref(),
            Some("direct")
        );
    }
}
