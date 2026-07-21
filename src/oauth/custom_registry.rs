use std::{error::Error, fmt};

use indexmap::IndexMap;
use serde_json::{Map, Value};

use super::{
    api_error::read_api_error_message,
    model_alias_merge::{CUSTOM_REGISTRY_MODEL_FIELDS, merge_refreshed_model_alias},
};

pub const CUSTOM_REGISTRY_DEFAULT_MAX_CONTEXT: u64 = 131_072;
pub const CUSTOM_REGISTRY_DEFAULT_CAPABILITIES: [&str; 1] = ["tool_use"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomRegistrySource {
    pub url: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomRegistryProviderType {
    Anthropic,
    OpenAi,
    OpenAiResponses,
    Kimi,
}

impl CustomRegistryProviderType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::OpenAiResponses => "openai_responses",
            Self::Kimi => "kimi",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CustomRegistryLimit {
    pub context: Option<u64>,
    pub output: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CustomRegistryModalities {
    pub input: Option<Vec<String>>,
    pub output: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomRegistryModelEntry {
    pub id: String,
    pub name: Option<String>,
    pub limit: Option<CustomRegistryLimit>,
    pub tool_call: Option<bool>,
    pub reasoning: Option<bool>,
    pub modalities: Option<CustomRegistryModalities>,
    pub support_efforts: Option<Vec<String>>,
    pub default_effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomRegistryProviderEntry {
    pub id: String,
    pub name: String,
    pub api: String,
    pub provider_type: CustomRegistryProviderType,
    pub environment: Option<Vec<String>>,
    pub models: IndexMap<String, CustomRegistryModelEntry>,
}

#[derive(Debug)]
pub enum CustomRegistryError {
    Api { status: u16, message: String },
    Request(reqwest::Error),
    Json(reqwest::Error),
    InvalidHeader(String),
    UnexpectedResponse(String),
}

impl CustomRegistryError {
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Api { status, .. } => Some(*status),
            _ => None,
        }
    }
}

impl fmt::Display for CustomRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api { message, .. }
            | Self::InvalidHeader(message)
            | Self::UnexpectedResponse(message) => formatter.write_str(message),
            Self::Request(error) | Self::Json(error) => error.fmt(formatter),
        }
    }
}

impl Error for CustomRegistryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Request(error) | Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

// Original:
//   packages/oauth/src/custom-registry.ts
//   fetchCustomRegistry()
pub async fn fetch_custom_registry(
    source: &CustomRegistrySource,
    user_agent: Option<&str>,
) -> Result<IndexMap<String, CustomRegistryProviderEntry>, CustomRegistryError> {
    let mut request = reqwest::Client::new()
        .get(&source.url)
        .header(reqwest::header::ACCEPT, "application/json");
    if let Some(user_agent) = user_agent {
        let value = reqwest::header::HeaderValue::from_str(user_agent)
            .map_err(|error| CustomRegistryError::InvalidHeader(error.to_string()))?;
        request = request.header(reqwest::header::USER_AGENT, value);
    }
    if !source.api_key.is_empty() {
        let value = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", source.api_key))
            .map_err(|error| CustomRegistryError::InvalidHeader(error.to_string()))?;
        request = request.header(reqwest::header::AUTHORIZATION, value);
    }

    let response = request.send().await.map_err(CustomRegistryError::Request)?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let fallback = format!(
            "Failed to fetch custom registry at {} (HTTP {status}).",
            source.url
        );
        let message = read_api_error_message(response, &fallback).await;
        return Err(CustomRegistryError::Api { status, message });
    }
    let payload = response
        .json::<Value>()
        .await
        .map_err(CustomRegistryError::Json)?;
    parse_custom_registry_payload(&payload, &source.url)
}

pub fn parse_custom_registry_payload(
    payload: &Value,
    source_url: &str,
) -> Result<IndexMap<String, CustomRegistryProviderEntry>, CustomRegistryError> {
    let Some(record) = payload.as_object() else {
        return Err(CustomRegistryError::UnexpectedResponse(format!(
            "Unexpected custom registry response at {source_url}: expected a JSON object keyed by provider id."
        )));
    };
    let mut entries = IndexMap::new();
    for (key, raw) in record {
        let Some(entry) = parse_provider_entry(raw) else {
            eprintln!(
                "[custom-registry] Skipping invalid entry \"{key}\" at {source_url}: missing required fields or unsupported type (id, name, api, type, models)."
            );
            continue;
        };
        entries.insert(key.clone(), entry);
    }
    Ok(entries)
}

// Original: capabilitiesFromCustomEntry()
pub fn capabilities_from_custom_entry(model: &CustomRegistryModelEntry) -> Vec<String> {
    let mut capabilities = Vec::new();
    if model.tool_call == Some(true) {
        capabilities.push("tool_use".to_owned());
    }
    if model.reasoning == Some(true)
        || model
            .support_efforts
            .as_ref()
            .is_some_and(|efforts| !efforts.is_empty())
    {
        capabilities.push("thinking".to_owned());
    }
    if has_modality(
        model
            .modalities
            .as_ref()
            .and_then(|value| value.input.as_ref()),
        "image",
    ) {
        capabilities.push("image_in".to_owned());
    }
    if has_modality(
        model
            .modalities
            .as_ref()
            .and_then(|value| value.input.as_ref()),
        "video",
    ) {
        capabilities.push("video_in".to_owned());
    }
    if has_modality(
        model
            .modalities
            .as_ref()
            .and_then(|value| value.output.as_ref()),
        "image",
    ) {
        capabilities.push("image_out".to_owned());
    }
    if has_modality(
        model
            .modalities
            .as_ref()
            .and_then(|value| value.output.as_ref()),
        "audio",
    ) {
        capabilities.push("audio_out".to_owned());
    }
    capabilities
}

// Original:
//   packages/oauth/src/custom-registry.ts
//   applyCustomRegistryProvider()
pub fn apply_custom_registry_provider(
    config: &mut Map<String, Value>,
    entry: &CustomRegistryProviderEntry,
    source: &CustomRegistrySource,
) {
    let provider_key = &entry.id;
    let providers = config
        .entry("providers")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(providers) = providers.as_object_mut() {
        providers.insert(
            provider_key.clone(),
            serde_json::json!({
                "type": entry.provider_type.as_str(),
                "baseUrl": entry.api,
                "apiKey": source.api_key,
                "source": {
                    "kind": "apiJson",
                    "url": source.url,
                    "apiKey": source.api_key,
                }
            }),
        );
    }

    let mut existing_models = config
        .remove("models")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let upstream_keys = entry
        .models
        .keys()
        .map(|model_key| format!("{provider_key}/{model_key}"))
        .collect::<std::collections::HashSet<_>>();
    existing_models.retain(|key, alias| {
        alias
            .get("provider")
            .and_then(Value::as_str)
            .is_none_or(|provider| provider != provider_key || upstream_keys.contains(key))
    });

    for (model_key, model) in &entry.models {
        let alias_key = format!("{provider_key}/{model_key}");
        let mut remote = Map::from_iter([
            ("provider".to_owned(), Value::String(provider_key.clone())),
            ("model".to_owned(), Value::String(model.id.clone())),
            (
                "maxContextSize".to_owned(),
                Value::from(resolve_max_context_size(model)),
            ),
            (
                "capabilities".to_owned(),
                Value::Array(
                    resolve_capabilities(model)
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                ),
            ),
            (
                "displayName".to_owned(),
                Value::String(model.name.clone().unwrap_or_else(|| model.id.clone())),
            ),
        ]);
        if let Some(efforts) = &model.support_efforts {
            remote.insert(
                "supportEfforts".to_owned(),
                Value::Array(efforts.iter().cloned().map(Value::String).collect()),
            );
        }
        if let Some(default_effort) = &model.default_effort {
            remote.insert(
                "defaultEffort".to_owned(),
                Value::String(default_effort.clone()),
            );
        }
        let merged = merge_refreshed_model_alias(
            existing_models.get(&alias_key).unwrap_or(&Value::Null),
            &remote,
            &CUSTOM_REGISTRY_MODEL_FIELDS,
        );
        existing_models.insert(alias_key, Value::Object(merged));
    }
    config.insert("models".to_owned(), Value::Object(existing_models));
}

// Original: removeCustomRegistryProvider()
pub fn remove_custom_registry_provider(config: &mut Map<String, Value>, provider_id: &str) {
    if let Some(providers) = config.get_mut("providers").and_then(Value::as_object_mut) {
        providers.remove(provider_id);
    }

    let current_default = config
        .get("defaultModel")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut removed_default = false;
    let mut models = config
        .remove("models")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    models.retain(|key, alias| {
        let remove = alias.get("provider").and_then(Value::as_str) == Some(provider_id);
        if remove && current_default.as_deref() == Some(key) {
            removed_default = true;
        }
        !remove
    });
    config.insert("models".to_owned(), Value::Object(models));
    if removed_default {
        config.remove("defaultModel");
    }
    if config.get("defaultProvider").and_then(Value::as_str) == Some(provider_id) {
        config.remove("defaultProvider");
    }
}

// Original: applyCustomRegistryEntries()
pub fn apply_custom_registry_entries(
    config: &mut Map<String, Value>,
    entries: &IndexMap<String, CustomRegistryProviderEntry>,
    source: &CustomRegistrySource,
) {
    let surviving = entries
        .values()
        .map(|entry| entry.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let disappeared = config
        .get("providers")
        .and_then(Value::as_object)
        .map(|providers| {
            providers
                .iter()
                .filter(|(provider_id, provider)| {
                    !surviving.contains(provider_id.as_str())
                        && provider
                            .get("source")
                            .and_then(Value::as_object)
                            .is_some_and(|existing_source| {
                                existing_source.get("kind").and_then(Value::as_str)
                                    == Some("apiJson")
                                    && existing_source.get("url").and_then(Value::as_str)
                                        == Some(source.url.as_str())
                            })
                })
                .map(|(provider_id, _)| provider_id.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for provider_id in disappeared {
        remove_custom_registry_provider(config, &provider_id);
    }

    for entry in entries.values() {
        let exists = config
            .get("providers")
            .and_then(Value::as_object)
            .is_some_and(|providers| providers.contains_key(&entry.id));
        if exists {
            remove_custom_registry_provider(config, &entry.id);
        }
        apply_custom_registry_provider(config, entry, source);
    }
}

fn has_rich_capability_hints(model: &CustomRegistryModelEntry) -> bool {
    model.tool_call.is_some()
        || model.reasoning.is_some()
        || model.modalities.is_some()
        || model.support_efforts.is_some()
}

fn resolve_max_context_size(model: &CustomRegistryModelEntry) -> u64 {
    model
        .limit
        .as_ref()
        .and_then(|limit| limit.context.or(limit.output))
        .unwrap_or(CUSTOM_REGISTRY_DEFAULT_MAX_CONTEXT)
}

fn resolve_capabilities(model: &CustomRegistryModelEntry) -> Vec<String> {
    if has_rich_capability_hints(model) {
        capabilities_from_custom_entry(model)
    } else {
        CUSTOM_REGISTRY_DEFAULT_CAPABILITIES
            .iter()
            .map(ToString::to_string)
            .collect()
    }
}

fn parse_provider_entry(value: &Value) -> Option<CustomRegistryProviderEntry> {
    let record = value.as_object()?;
    let id = nonempty_string(record.get("id"))?;
    let name = nonempty_string(record.get("name"))?;
    let api = nonempty_string(record.get("api"))?;
    let provider_type = parse_provider_type(record.get("type")?.as_str()?)?;
    let raw_models = record.get("models")?.as_object()?;
    let mut models = IndexMap::new();
    for (key, raw) in raw_models {
        if let Some(model) = parse_model_entry(raw) {
            models.insert(key.clone(), model);
        }
    }
    Some(CustomRegistryProviderEntry {
        id,
        name,
        api,
        provider_type,
        environment: string_array(record.get("env")),
        models,
    })
}

fn parse_model_entry(value: &Value) -> Option<CustomRegistryModelEntry> {
    let record = value.as_object()?;
    let id = nonempty_string(record.get("id"))?;
    let limit = record
        .get("limit")
        .and_then(Value::as_object)
        .and_then(|limit| {
            let context = positive_floored_integer(limit.get("context"));
            let output = positive_floored_integer(limit.get("output"));
            (context.is_some() || output.is_some())
                .then_some(CustomRegistryLimit { context, output })
        });
    let modalities = record
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
        name: nonempty_string(record.get("name")),
        limit,
        tool_call: record.get("tool_call").and_then(Value::as_bool),
        reasoning: record.get("reasoning").and_then(Value::as_bool),
        modalities,
        support_efforts: string_array(record.get("support_efforts")),
        default_effort: nonempty_string(record.get("default_effort")),
    })
}

fn parse_provider_type(value: &str) -> Option<CustomRegistryProviderType> {
    match value {
        "anthropic" => Some(CustomRegistryProviderType::Anthropic),
        "openai" => Some(CustomRegistryProviderType::OpenAi),
        "openai_responses" => Some(CustomRegistryProviderType::OpenAiResponses),
        "kimi" => Some(CustomRegistryProviderType::Kimi),
        _ => None,
    }
}

fn nonempty_string(value: Option<&Value>) -> Option<String> {
    value?
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn string_array(value: Option<&Value>) -> Option<Vec<String>> {
    let values = match value {
        None | Some(Value::Null) => return None,
        Some(Value::Array(values)) => values,
        Some(_) => return None,
    };
    values
        .iter()
        .map(|value| value.as_str().map(str::to_owned))
        .collect()
}

fn positive_floored_integer(value: Option<&Value>) -> Option<u64> {
    let number = value?.as_f64()?;
    (number.is_finite() && number > 0.0 && number.floor() <= u64::MAX as f64)
        .then_some(number.floor() as u64)
}

fn has_modality(values: Option<&Vec<String>>, expected: &str) -> bool {
    values.is_some_and(|values| values.iter().any(|value| value == expected))
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

    fn fake_server(
        status: u16,
        body: &str,
    ) -> (String, Arc<Mutex<String>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind registry server");
        let address = listener.local_addr().expect("registry server address");
        let request = Arc::new(Mutex::new(String::new()));
        let recorded = Arc::clone(&request);
        let body = body.to_owned();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept registry request");
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 4_096];
            loop {
                let count = stream.read(&mut buffer).expect("read registry request");
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
                if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            *recorded.lock().expect("request lock") = String::from_utf8_lossy(&bytes).into_owned();
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write registry response");
        });
        (format!("http://{address}/api.json"), request, handle)
    }

    fn model(id: &str) -> CustomRegistryModelEntry {
        CustomRegistryModelEntry {
            id: id.to_owned(),
            name: None,
            limit: None,
            tool_call: None,
            reasoning: None,
            modalities: None,
            support_efforts: None,
            default_effort: None,
        }
    }

    fn provider(
        id: &str,
        models: &[(&str, CustomRegistryModelEntry)],
    ) -> CustomRegistryProviderEntry {
        CustomRegistryProviderEntry {
            id: id.to_owned(),
            name: format!("Provider {id}"),
            api: format!("https://{id}.example/v1"),
            provider_type: CustomRegistryProviderType::OpenAi,
            environment: None,
            models: models
                .iter()
                .map(|(key, model)| ((*key).to_owned(), model.clone()))
                .collect(),
        }
    }

    #[test]
    fn parses_valid_entries_skips_invalid_providers_and_models() {
        let entries = parse_custom_registry_payload(
            &serde_json::json!({
                "valid": {
                    "id": "provider",
                    "name": "Provider",
                    "api": "https://api.example/v1",
                    "type": "openai",
                    "env": ["API_KEY"],
                    "models": {
                        "good": {
                            "id": "model-id",
                            "name": "Model",
                            "limit": { "context": 131072.9, "output": -1 },
                            "tool_call": true,
                            "support_efforts": ["low", "high"],
                            "default_effort": "high",
                            "modalities": { "input": ["text", "image"], "output": ["text"] }
                        },
                        "bad": { "name": "missing id" }
                    }
                },
                "future": {
                    "id": "future", "name": "Future", "api": "x",
                    "type": "unsupported", "models": {}
                }
            }),
            "https://registry.example/api.json",
        )
        .expect("registry");

        assert_eq!(entries.len(), 1);
        let provider = &entries["valid"];
        assert_eq!(provider.id, "provider");
        assert_eq!(provider.provider_type, CustomRegistryProviderType::OpenAi);
        assert_eq!(provider.environment, Some(vec!["API_KEY".to_owned()]));
        assert_eq!(provider.models.len(), 1);
        let model = &provider.models["good"];
        assert_eq!(
            model.limit.as_ref().and_then(|limit| limit.context),
            Some(131_072)
        );
        assert_eq!(model.limit.as_ref().and_then(|limit| limit.output), None);
        assert_eq!(
            model.support_efforts,
            Some(vec!["low".to_owned(), "high".to_owned()])
        );
    }

    #[test]
    fn derives_capabilities_in_original_insertion_order() {
        let model = CustomRegistryModelEntry {
            id: "model".to_owned(),
            name: None,
            limit: None,
            tool_call: Some(true),
            reasoning: Some(false),
            modalities: Some(CustomRegistryModalities {
                input: Some(vec!["video".to_owned(), "image".to_owned()]),
                output: Some(vec!["audio".to_owned(), "image".to_owned()]),
            }),
            support_efforts: Some(vec!["high".to_owned()]),
            default_effort: None,
        };
        assert_eq!(
            capabilities_from_custom_entry(&model),
            [
                "tool_use",
                "thinking",
                "image_in",
                "video_in",
                "image_out",
                "audio_out"
            ]
        );
    }

    #[tokio::test]
    async fn fetches_registry_with_accept_auth_and_user_agent_headers() {
        let (url, request, server) = fake_server(
            200,
            r#"{"p":{"id":"p","name":"Provider","api":"https://api","type":"kimi","models":{}}}"#,
        );
        let entries = fetch_custom_registry(
            &CustomRegistrySource {
                url,
                api_key: "secret".to_owned(),
            },
            Some("kimi-code-cli/1.2.3"),
        )
        .await
        .expect("registry");
        server.join().expect("registry server thread");
        assert_eq!(entries.len(), 1);
        let request = request.lock().expect("request lock").to_ascii_lowercase();
        assert!(request.contains("accept: application/json"));
        assert!(request.contains("authorization: bearer secret"));
        assert!(request.contains("user-agent: kimi-code-cli/1.2.3"));
    }

    #[tokio::test]
    async fn empty_api_key_omits_authorization_and_api_errors_keep_status() {
        let (url, request, server) = fake_server(401, r#"{"error":{"message":"denied"}}"#);
        let error = fetch_custom_registry(
            &CustomRegistrySource {
                url,
                api_key: String::new(),
            },
            None,
        )
        .await
        .expect_err("registry auth error");
        server.join().expect("registry server thread");
        assert_eq!(error.status(), Some(401));
        assert_eq!(error.to_string(), "denied");
        assert!(
            !request
                .lock()
                .expect("request lock")
                .to_ascii_lowercase()
                .contains("authorization:")
        );
    }

    #[test]
    fn rejects_non_object_payload_with_exact_context() {
        let error = parse_custom_registry_payload(
            &serde_json::json!([]),
            "https://registry.example/api.json",
        )
        .expect_err("non-object");
        assert_eq!(
            error.to_string(),
            "Unexpected custom registry response at https://registry.example/api.json: expected a JSON object keyed by provider id."
        );
    }

    #[test]
    fn applies_provider_with_fallbacks_and_preserves_user_fields() {
        let mut config = serde_json::json!({
            "providers": { "other": { "type": "kimi" } },
            "models": {
                "registry/model": {
                    "provider": "registry",
                    "model": "old",
                    "maxContextSize": 1,
                    "userNote": "keep",
                    "overrides": { "maxContextSize": 4096 },
                    "supportEfforts": ["stale"],
                    "defaultEffort": "stale"
                },
                "registry/stale": { "provider": "registry", "model": "stale" },
                "other/model": { "provider": "other", "model": "model" }
            }
        });
        let entry = provider("registry", &[("model", model("upstream-id"))]);
        let source = CustomRegistrySource {
            url: "https://registry.example/api.json".to_owned(),
            api_key: "secret".to_owned(),
        };

        apply_custom_registry_provider(
            config.as_object_mut().expect("config object"),
            &entry,
            &source,
        );

        assert_eq!(
            config["providers"]["registry"],
            serde_json::json!({
                "type": "openai",
                "baseUrl": "https://registry.example/v1",
                "apiKey": "secret",
                "source": {
                    "kind": "apiJson",
                    "url": "https://registry.example/api.json",
                    "apiKey": "secret"
                }
            })
        );
        let alias = &config["models"]["registry/model"];
        assert_eq!(alias["model"], "upstream-id");
        assert_eq!(alias["maxContextSize"], 131_072);
        assert_eq!(alias["capabilities"], serde_json::json!(["tool_use"]));
        assert_eq!(alias["displayName"], "upstream-id");
        assert_eq!(alias["userNote"], "keep");
        assert_eq!(
            alias["overrides"],
            serde_json::json!({ "maxContextSize": 4096 })
        );
        assert!(alias.get("supportEfforts").is_none());
        assert!(alias.get("defaultEffort").is_none());
        assert!(config["models"].get("registry/stale").is_none());
        assert!(config["models"].get("other/model").is_some());
    }

    #[test]
    fn rich_hints_and_limits_control_alias_fields() {
        let mut rich = model("rich-id");
        rich.name = Some("Rich Model".to_owned());
        rich.limit = Some(CustomRegistryLimit {
            context: None,
            output: Some(65_536),
        });
        rich.tool_call = Some(false);
        rich.reasoning = Some(true);
        rich.modalities = Some(CustomRegistryModalities {
            input: Some(vec!["image".to_owned()]),
            output: None,
        });
        rich.support_efforts = Some(vec!["low".to_owned(), "high".to_owned()]);
        rich.default_effort = Some("high".to_owned());
        let entry = provider("registry", &[("rich", rich)]);
        let mut config = serde_json::json!({ "providers": {} });

        apply_custom_registry_provider(
            config.as_object_mut().expect("config object"),
            &entry,
            &CustomRegistrySource {
                url: "https://registry.example/api.json".to_owned(),
                api_key: String::new(),
            },
        );

        let alias = &config["models"]["registry/rich"];
        assert_eq!(alias["maxContextSize"], 65_536);
        assert_eq!(alias["displayName"], "Rich Model");
        assert_eq!(
            alias["capabilities"],
            serde_json::json!(["thinking", "image_in"])
        );
        assert_eq!(alias["supportEfforts"], serde_json::json!(["low", "high"]));
        assert_eq!(alias["defaultEffort"], "high");
    }

    #[test]
    fn remove_provider_clears_only_matching_defaults_and_aliases() {
        let mut config = serde_json::json!({
            "providers": { "registry": {}, "other": {} },
            "models": {
                "registry/model": { "provider": "registry" },
                "other/model": { "provider": "other" }
            },
            "defaultModel": "registry/model",
            "defaultProvider": "registry"
        });
        remove_custom_registry_provider(config.as_object_mut().expect("config object"), "registry");
        assert!(config["providers"].get("registry").is_none());
        assert!(config["providers"].get("other").is_some());
        assert!(config["models"].get("registry/model").is_none());
        assert!(config["models"].get("other/model").is_some());
        assert!(config.get("defaultModel").is_none());
        assert!(config.get("defaultProvider").is_none());
    }

    #[test]
    fn batch_reimport_keeps_all_survivors_and_removes_disappeared_same_source() {
        let source = CustomRegistrySource {
            url: "https://registry.example/api.json".to_owned(),
            api_key: "rotated".to_owned(),
        };
        let old_source = serde_json::json!({
            "kind": "apiJson",
            "url": "https://registry.example/api.json",
            "apiKey": "old"
        });
        let mut config = serde_json::json!({
            "providers": {
                "first": { "source": old_source },
                "removed": { "source": old_source },
                "foreign": {
                    "source": { "kind": "apiJson", "url": "https://other.example/api.json", "apiKey": "old" }
                }
            },
            "models": {
                "first/old": { "provider": "first" },
                "removed/model": { "provider": "removed" },
                "foreign/model": { "provider": "foreign" }
            },
            "defaultModel": "removed/model"
        });
        let entries = IndexMap::from([
            (
                "one".to_owned(),
                provider("first", &[("new", model("new"))]),
            ),
            (
                "two".to_owned(),
                provider("second", &[("model", model("model"))]),
            ),
        ]);

        apply_custom_registry_entries(
            config.as_object_mut().expect("config object"),
            &entries,
            &source,
        );

        assert!(config["providers"].get("first").is_some());
        assert!(config["providers"].get("second").is_some());
        assert!(config["providers"].get("removed").is_none());
        assert!(config["providers"].get("foreign").is_some());
        assert!(config["models"].get("first/new").is_some());
        assert!(config["models"].get("second/model").is_some());
        assert!(config["models"].get("first/old").is_none());
        assert!(config["models"].get("removed/model").is_none());
        assert!(config["models"].get("foreign/model").is_some());
        assert!(config.get("defaultModel").is_none());
    }
}
