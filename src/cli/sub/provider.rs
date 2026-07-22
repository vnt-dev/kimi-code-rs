use std::{collections::BTreeMap, error::Error, fmt};

use async_trait::async_trait;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::cli::commands::{CatalogCommand, ProviderCommand};

#[derive(Debug)]
pub struct ProviderError(Box<dyn Error + Send + Sync>);

impl ProviderError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for ProviderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

impl From<serde_json::Error> for ProviderError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error)
    }
}

#[derive(Debug)]
pub enum ProviderCommandError {
    Runtime(ProviderError),
    Exit(i32),
}

impl fmt::Display for ProviderCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(error) => error.fmt(formatter),
            Self::Exit(code) => write!(formatter, "provider command requested exit code {code}"),
        }
    }
}

impl Error for ProviderCommandError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Runtime(error) => Some(error),
            Self::Exit(_) => None,
        }
    }
}

impl From<ProviderError> for ProviderCommandError {
    fn from(error: ProviderError) -> Self {
        Self::Runtime(error)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDefinition {
    #[serde(rename = "type")]
    pub provider_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Map<String, Value>>,
    #[serde(flatten)]
    pub additional_fields: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDefinition {
    pub provider: String,
    pub model: String,
    #[serde(flatten)]
    pub additional_fields: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderDefinition>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, ModelDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Value>,
    #[serde(flatten)]
    pub additional_fields: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfigPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub providers: Option<BTreeMap<String, ProviderDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<BTreeMap<String, ModelDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Value>,
}

#[async_trait]
pub trait ProviderRuntime: Send + Sync {
    async fn ensure_config_file(&self) -> Result<(), ProviderError>;
    async fn get_config(&self) -> Result<ProviderConfig, ProviderError>;
    async fn remove_provider(&self, provider_id: &str) -> Result<ProviderConfig, ProviderError>;
    async fn set_config(
        &self,
        patch: &ProviderConfigPatch,
    ) -> Result<ProviderConfig, ProviderError>;
    fn write_stdout(&self, text: &str);
    fn write_stderr(&self, text: &str);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomRegistrySource {
    pub kind: String,
    pub url: String,
    pub api_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CustomRegistryLimit {
    pub context: Option<u64>,
    pub output: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CustomRegistryModalities {
    pub input: Option<Vec<String>>,
    pub output: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomRegistryProviderEntry {
    pub id: String,
    pub name: String,
    pub api: String,
    #[serde(rename = "type")]
    pub provider_type: String,
    pub env: Option<Vec<String>>,
    pub models: BTreeMap<String, CustomRegistryModelEntry>,
}

#[derive(Debug)]
pub struct RegistryFetchError {
    pub status: Option<u16>,
    source: ProviderError,
}

impl RegistryFetchError {
    pub fn new(error: impl Error + Send + Sync + 'static, status: Option<u16>) -> Self {
        Self {
            status,
            source: ProviderError::new(error),
        }
    }
}

impl fmt::Display for RegistryFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.source.fmt(formatter)
    }
}

impl Error for RegistryFetchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

#[async_trait]
pub trait ProviderRegistryRuntime: ProviderRuntime {
    async fn fetch_custom_registry(
        &self,
        source: &CustomRegistrySource,
    ) -> Result<Vec<CustomRegistryProviderEntry>, RegistryFetchError>;
}

// Original:
//   apps/kimi-code/src/cli/sub/provider.ts
//   handleProviderAdd()
pub async fn handle_provider_add(
    runtime: &dyn ProviderRegistryRuntime,
    url: &str,
    flag_api_key: Option<&str>,
    environment_api_key: Option<&str>,
) -> Result<(), ProviderCommandError> {
    let Some(api_key) = resolve_api_key(flag_api_key, environment_api_key) else {
        runtime
            .write_stderr("Missing API key. Pass --api-key <key> or set KIMI_REGISTRY_API_KEY.\n");
        return Err(ProviderCommandError::Exit(1));
    };

    let trimmed_url = url.trim();
    if trimmed_url.is_empty() {
        runtime.write_stderr("Registry URL is required.\n");
        return Err(ProviderCommandError::Exit(1));
    }

    let source = CustomRegistrySource {
        kind: "apiJson".to_owned(),
        url: trimmed_url.to_owned(),
        api_key,
    };
    runtime.ensure_config_file().await?;
    let entries = match runtime.fetch_custom_registry(&source).await {
        Ok(entries) => entries,
        Err(error) => {
            let suffix = error
                .status
                .map_or_else(String::new, |status| format!(" (HTTP {status})"));
            runtime.write_stderr(&format!("Failed to fetch registry{suffix}: {error}\n"));
            return Err(ProviderCommandError::Exit(1));
        }
    };

    if entries.is_empty() {
        runtime.write_stderr(&format!(
            "Registry at {trimmed_url} contained no usable providers.\n"
        ));
        return Err(ProviderCommandError::Exit(1));
    }

    let mut config = runtime.get_config().await?;
    let stale_ids: Vec<String> = entries
        .iter()
        .filter(|entry| config.providers.contains_key(&entry.id))
        .map(|entry| entry.id.clone())
        .collect();
    for id in stale_ids {
        config = runtime.remove_provider(&id).await?;
    }

    let mut added_provider_ids = Vec::with_capacity(entries.len());
    let mut model_count = 0_usize;
    for entry in entries {
        model_count += entry.models.len();
        added_provider_ids.push(entry.id.clone());
        apply_custom_registry_provider(&mut config, &entry, &source);
    }

    runtime
        .set_config(&ProviderConfigPatch {
            providers: Some(config.providers.clone()),
            models: Some(config.models.clone()),
            ..ProviderConfigPatch::default()
        })
        .await?;
    runtime.write_stdout(&format!(
        "Imported {} provider{} ({} model{}) from {trimmed_url}:\n",
        added_provider_ids.len(),
        if added_provider_ids.len() == 1 {
            ""
        } else {
            "s"
        },
        model_count,
        if model_count == 1 { "" } else { "s" },
    ));
    for id in added_provider_ids {
        runtime.write_stdout(&format!("  - {id}\n"));
    }
    Ok(())
}

const CUSTOM_REGISTRY_DEFAULT_MAX_CONTEXT: u64 = 131_072;

// Original:
//   packages/oauth/src/custom-registry.ts
//   applyCustomRegistryProvider()
pub fn apply_custom_registry_provider(
    config: &mut ProviderConfig,
    entry: &CustomRegistryProviderEntry,
    source: &CustomRegistrySource,
) {
    config.providers.insert(
        entry.id.clone(),
        ProviderDefinition {
            provider_type: entry.provider_type.clone(),
            base_url: Some(entry.api.clone()),
            api_key: Some(source.api_key.clone()),
            oauth: None,
            source: Some(Map::from_iter([
                ("kind".to_owned(), Value::String(source.kind.clone())),
                ("url".to_owned(), Value::String(source.url.clone())),
                ("apiKey".to_owned(), Value::String(source.api_key.clone())),
            ])),
            additional_fields: Map::new(),
        },
    );

    let upstream_keys: Vec<String> = entry
        .models
        .keys()
        .map(|model_key| format!("{}/{model_key}", entry.id))
        .collect();
    config.models.retain(|alias, model| {
        model.provider != entry.id || upstream_keys.iter().any(|key| key == alias)
    });

    for (model_key, model) in &entry.models {
        let alias_key = format!("{}/{model_key}", entry.id);
        let mut additional_fields = config
            .models
            .remove(&alias_key)
            .map_or_else(Map::new, |existing| existing.additional_fields);
        for managed_field in [
            "maxContextSize",
            "capabilities",
            "displayName",
            "supportEfforts",
            "defaultEffort",
        ] {
            additional_fields.remove(managed_field);
        }

        additional_fields.insert(
            "maxContextSize".to_owned(),
            Value::from(resolve_max_context_size(model)),
        );
        additional_fields.insert(
            "capabilities".to_owned(),
            Value::from(
                resolve_capabilities(model)
                    .into_iter()
                    .map(Value::String)
                    .collect::<Vec<_>>(),
            ),
        );
        let display_name = model
            .name
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or(&model.id);
        additional_fields.insert(
            "displayName".to_owned(),
            Value::String(display_name.to_owned()),
        );
        if let Some(support_efforts) = &model.support_efforts {
            additional_fields.insert(
                "supportEfforts".to_owned(),
                serde_json::json!(support_efforts),
            );
        }
        if let Some(default_effort) = &model.default_effort {
            additional_fields.insert(
                "defaultEffort".to_owned(),
                Value::String(default_effort.clone()),
            );
        }

        config.models.insert(
            alias_key,
            ModelDefinition {
                provider: entry.id.clone(),
                model: model.id.clone(),
                additional_fields,
            },
        );
    }
}

fn resolve_max_context_size(model: &CustomRegistryModelEntry) -> u64 {
    model
        .limit
        .as_ref()
        .and_then(|limit| {
            limit
                .context
                .filter(|value| *value > 0)
                .or_else(|| limit.output.filter(|value| *value > 0))
        })
        .unwrap_or(CUSTOM_REGISTRY_DEFAULT_MAX_CONTEXT)
}

fn resolve_capabilities(model: &CustomRegistryModelEntry) -> Vec<String> {
    let has_rich_hints = model.tool_call.is_some()
        || model.reasoning.is_some()
        || model.modalities.is_some()
        || model.support_efforts.is_some();
    if !has_rich_hints {
        return vec!["tool_use".to_owned()];
    }

    let mut capabilities = Vec::new();
    if model.tool_call == Some(true) {
        capabilities.push("tool_use".to_owned());
    }
    if model.reasoning == Some(true)
        || model
            .support_efforts
            .as_ref()
            .is_some_and(|values| !values.is_empty())
    {
        capabilities.push("thinking".to_owned());
    }
    let input = model
        .modalities
        .as_ref()
        .and_then(|modalities| modalities.input.as_ref());
    let output = model
        .modalities
        .as_ref()
        .and_then(|modalities| modalities.output.as_ref());
    for (values, name, capability) in [
        (input, "image", "image_in"),
        (input, "video", "video_in"),
        (output, "image", "image_out"),
        (output, "audio", "audio_out"),
    ] {
        if values.is_some_and(|values| values.iter().any(|value| value == name)) {
            capabilities.push(capability.to_owned());
        }
    }
    capabilities
}

pub const DEFAULT_CATALOG_URL: &str = "https://models.dev/api.json";
pub const KIMI_REGISTRY_API_KEY_ENV: &str = "KIMI_REGISTRY_API_KEY";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogLimit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogModalities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogModelEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<CatalogLimit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamically_loaded_tools: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interleaved: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modalities: Option<CatalogModalities>,
    #[serde(flatten)]
    pub additional_fields: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogProviderEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub npm: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<IndexMap<String, CatalogModelEntry>>,
    #[serde(flatten)]
    pub additional_fields: Map<String, Value>,
}

pub type Catalog = IndexMap<String, CatalogProviderEntry>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireType {
    Anthropic,
    OpenAi,
    Kimi,
    GoogleGenAi,
    OpenAiResponses,
    VertexAi,
}

impl WireType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Kimi => "kimi",
            Self::GoogleGenAi => "google-genai",
            Self::OpenAiResponses => "openai_responses",
            Self::VertexAi => "vertexai",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogCapability {
    pub image_in: bool,
    pub video_in: bool,
    pub audio_in: bool,
    pub thinking: bool,
    pub tool_use: bool,
    pub max_context_tokens: u64,
    pub dynamically_loaded_tools: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_size: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_key: Option<String>,
    pub capability: CatalogCapability,
}

#[derive(Debug)]
pub struct CatalogFetchError {
    pub status: Option<u16>,
    source: ProviderError,
}

impl CatalogFetchError {
    pub fn new(error: impl Error + Send + Sync + 'static, status: Option<u16>) -> Self {
        Self {
            status,
            source: ProviderError::new(error),
        }
    }
}

impl fmt::Display for CatalogFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.source.fmt(formatter)
    }
}

impl Error for CatalogFetchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

#[async_trait]
pub trait ProviderCatalogRuntime: ProviderRuntime {
    async fn fetch_catalog(&self, url: &str) -> Result<Catalog, CatalogFetchError>;
}

pub trait FullProviderRuntime: ProviderRegistryRuntime + ProviderCatalogRuntime {}

impl<T> FullProviderRuntime for T where T: ProviderRegistryRuntime + ProviderCatalogRuntime {}

// Original:
//   apps/kimi-code/src/cli/sub/provider.ts
//   registerProviderCommand(), runAction()
//
// Rust adaptation:
//   Clap performs registration declaratively in cli/commands.rs. This method
//   preserves the original action routing and last-resort error boundary.
pub async fn run_provider_command<R>(
    runtime: &R,
    command: &ProviderCommand,
    environment_api_key: Option<&str>,
) -> i32
where
    R: FullProviderRuntime,
{
    let result = match command {
        ProviderCommand::Add { url, api_key } => {
            handle_provider_add(runtime, url, api_key.as_deref(), environment_api_key).await
        }
        ProviderCommand::Remove { provider_id } => {
            handle_provider_remove(runtime, provider_id).await
        }
        ProviderCommand::List { json } => handle_provider_list(runtime, *json).await,
        ProviderCommand::Catalog { command } => match command {
            CatalogCommand::List {
                provider_id,
                filter,
                url,
                json,
            } => {
                handle_catalog_list(
                    runtime,
                    provider_id.as_deref(),
                    *json,
                    filter.as_deref(),
                    url.as_deref(),
                )
                .await
            }
            CatalogCommand::Add {
                provider_id,
                api_key,
                default_model,
                url,
            } => {
                handle_catalog_add(
                    runtime,
                    provider_id,
                    api_key.as_deref(),
                    environment_api_key,
                    default_model.as_deref(),
                    url.as_deref(),
                )
                .await
            }
        },
    };

    match result {
        Ok(()) => 0,
        Err(ProviderCommandError::Exit(code)) => code,
        Err(ProviderCommandError::Runtime(error)) => {
            runtime.write_stderr(&format!("{error}\n"));
            1
        }
    }
}

// Original:
//   apps/kimi-code/src/cli/sub/provider.ts
//   handleCatalogAdd()
pub async fn handle_catalog_add(
    runtime: &dyn ProviderCatalogRuntime,
    provider_id: &str,
    flag_api_key: Option<&str>,
    environment_api_key: Option<&str>,
    default_model: Option<&str>,
    url: Option<&str>,
) -> Result<(), ProviderCommandError> {
    let Some(api_key) = resolve_api_key(flag_api_key, environment_api_key) else {
        runtime
            .write_stderr("Missing API key. Pass --api-key <key> or set KIMI_REGISTRY_API_KEY.\n");
        return Err(ProviderCommandError::Exit(1));
    };
    let url = url.unwrap_or(DEFAULT_CATALOG_URL);
    let catalog = match runtime.fetch_catalog(url).await {
        Ok(catalog) => catalog,
        Err(error) => {
            let suffix = error
                .status
                .map_or_else(String::new, |status| format!(" (HTTP {status})"));
            runtime.write_stderr(&format!(
                "Failed to fetch catalog from {url}{suffix}: {error}\n"
            ));
            return Err(ProviderCommandError::Exit(1));
        }
    };
    let Some(entry) = catalog.get(provider_id) else {
        runtime.write_stderr(&format!(
            "Provider \"{provider_id}\" not found in catalog at {url}.\n"
        ));
        return Err(ProviderCommandError::Exit(1));
    };
    let Some(wire) = infer_wire_type(entry) else {
        runtime.write_stderr(&format!(
            "Provider \"{provider_id}\" has an unsupported wire type in the catalog.\n"
        ));
        return Err(ProviderCommandError::Exit(1));
    };
    let models = catalog_provider_models(entry);
    if models.is_empty() {
        runtime.write_stderr(&format!(
            "Provider \"{provider_id}\" lists no usable models in this catalog.\n"
        ));
        return Err(ProviderCommandError::Exit(1));
    }
    if let Some(default_model) = default_model
        && !models.iter().any(|model| model.id == default_model)
    {
        runtime.write_stderr(&format!(
            "Model \"{default_model}\" is not in provider \"{provider_id}\". Run \"kimi provider catalog list {provider_id}\" to see available ids.\n"
        ));
        return Err(ProviderCommandError::Exit(1));
    }

    runtime.ensure_config_file().await?;
    let mut config = runtime.get_config().await?;
    let previous_default_model = config.default_model.clone();
    let previous_thinking = config.thinking.clone();
    if config.providers.contains_key(provider_id) {
        config = runtime.remove_provider(provider_id).await?;
    }
    apply_catalog_provider(
        &mut config,
        ApplyCatalogProviderOptions {
            provider_id,
            wire,
            base_url: catalog_base_url(entry, wire),
            api_key: &api_key,
            models: &models,
            selected_model_id: default_model.unwrap_or_default(),
            thinking: false,
        },
    );
    if default_model.is_none() {
        config.default_model =
            previous_default_model.filter(|alias| config.models.contains_key(alias));
    }
    config.thinking = previous_thinking;
    runtime
        .set_config(&ProviderConfigPatch {
            providers: Some(config.providers.clone()),
            models: Some(config.models.clone()),
            default_model: config.default_model.clone(),
            thinking: config.thinking.clone(),
        })
        .await?;

    let display_name = entry.name.as_deref().unwrap_or(provider_id);
    runtime.write_stdout(&format!(
        "Imported {display_name} ({provider_id}) with {} model{} from {url}.\n",
        models.len(),
        if models.len() == 1 { "" } else { "s" }
    ));
    if let Some(default_model) = default_model {
        runtime.write_stdout(&format!(
            "Default model set to {provider_id}/{default_model}.\n"
        ));
    }
    Ok(())
}

// Original:
//   packages/kosong/src/catalog.ts
//   catalogBaseUrl()
pub fn catalog_base_url(entry: &CatalogProviderEntry, wire: WireType) -> Option<String> {
    let api = entry.api.as_deref().filter(|api| !api.is_empty())?;
    if wire == WireType::Anthropic {
        if let Some(base) = api.strip_suffix("/v1/") {
            return Some(base.to_owned());
        }
        if let Some(base) = api.strip_suffix("/v1") {
            return Some(base.to_owned());
        }
    }
    Some(api.to_owned())
}

// Original:
//   packages/node-sdk/src/catalog.ts
//   applyCatalogProvider()
pub struct ApplyCatalogProviderOptions<'a> {
    pub provider_id: &'a str,
    pub wire: WireType,
    pub base_url: Option<String>,
    pub api_key: &'a str,
    pub models: &'a [CatalogModel],
    pub selected_model_id: &'a str,
    pub thinking: bool,
}

pub fn apply_catalog_provider(
    config: &mut ProviderConfig,
    options: ApplyCatalogProviderOptions<'_>,
) -> String {
    let provider_id = options.provider_id;
    config.providers.insert(
        provider_id.to_owned(),
        ProviderDefinition {
            provider_type: options.wire.as_str().to_owned(),
            base_url: options.base_url,
            api_key: Some(options.api_key.to_owned()),
            oauth: None,
            source: None,
            additional_fields: Map::new(),
        },
    );
    config
        .models
        .retain(|_, model| model.provider != provider_id);
    for model in options.models {
        config.models.insert(
            format!("{provider_id}/{}", model.id),
            catalog_model_to_alias(provider_id, model),
        );
    }
    let default_model = format!("{provider_id}/{}", options.selected_model_id);
    config.default_model = Some(default_model.clone());
    let mut thinking_config = match config.thinking.take() {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    };
    thinking_config.insert("enabled".to_owned(), Value::Bool(options.thinking));
    config.thinking = Some(Value::Object(thinking_config));
    default_model
}

fn catalog_model_to_alias(provider_id: &str, model: &CatalogModel) -> ModelDefinition {
    let mut capabilities = Vec::new();
    for (enabled, capability) in [
        (model.capability.image_in, "image_in"),
        (model.capability.video_in, "video_in"),
        (model.capability.audio_in, "audio_in"),
        (model.capability.thinking, "thinking"),
        (model.capability.tool_use, "tool_use"),
        (
            model.capability.dynamically_loaded_tools,
            "dynamically_loaded_tools",
        ),
    ] {
        if enabled {
            capabilities.push(capability);
        }
    }
    let mut additional_fields = Map::from_iter([(
        "maxContextSize".to_owned(),
        Value::from(model.capability.max_context_tokens),
    )]);
    if !capabilities.is_empty() {
        additional_fields.insert("capabilities".to_owned(), serde_json::json!(capabilities));
    }
    if let Some(max_output_size) = model.max_output_size {
        additional_fields.insert(
            "maxOutputSize".to_owned(),
            serde_json::json!(max_output_size),
        );
    }
    if let Some(name) = &model.name {
        additional_fields.insert("displayName".to_owned(), Value::String(name.clone()));
    }
    if let Some(reasoning_key) = &model.reasoning_key {
        additional_fields.insert(
            "reasoningKey".to_owned(),
            Value::String(reasoning_key.clone()),
        );
    }
    ModelDefinition {
        provider: provider_id.to_owned(),
        model: model.id.clone(),
        additional_fields,
    }
}

// Original:
//   apps/kimi-code/src/cli/sub/provider.ts
//   handleCatalogList()
pub async fn handle_catalog_list(
    runtime: &dyn ProviderCatalogRuntime,
    provider_id: Option<&str>,
    json: bool,
    filter: Option<&str>,
    url: Option<&str>,
) -> Result<(), ProviderCommandError> {
    let url = url.unwrap_or(DEFAULT_CATALOG_URL);
    let catalog = match runtime.fetch_catalog(url).await {
        Ok(catalog) => catalog,
        Err(error) => {
            let suffix = error
                .status
                .map_or_else(String::new, |status| format!(" (HTTP {status})"));
            runtime.write_stderr(&format!(
                "Failed to fetch catalog from {url}{suffix}: {error}\n"
            ));
            return Err(ProviderCommandError::Exit(1));
        }
    };

    if let Some(provider_id) = provider_id {
        let Some(entry) = catalog.get(provider_id) else {
            runtime.write_stderr(&format!(
                "Provider \"{provider_id}\" not found in catalog at {url}.\n"
            ));
            return Err(ProviderCommandError::Exit(1));
        };
        let models = catalog_provider_models(entry);
        if json {
            #[derive(Serialize)]
            #[serde(rename_all = "camelCase")]
            struct CatalogProviderModels<'a> {
                provider_id: &'a str,
                name: &'a str,
                models: &'a [CatalogModel],
            }
            let output = serde_json::to_string_pretty(&CatalogProviderModels {
                provider_id,
                name: entry.name.as_deref().unwrap_or(provider_id),
                models: &models,
            })
            .map_err(ProviderError::from)?;
            runtime.write_stdout(&format!("{output}\n"));
            return Ok(());
        }
        if models.is_empty() {
            runtime.write_stdout(&format!(
                "Provider \"{provider_id}\" lists no usable models in this catalog.\n"
            ));
            return Ok(());
        }
        runtime.write_stdout(&format!(
            "{} ({provider_id})\n",
            entry.name.as_deref().unwrap_or(provider_id)
        ));
        for model in models {
            let mut capabilities = Vec::new();
            if model.capability.tool_use {
                capabilities.push("tool_use");
            }
            if model.capability.thinking {
                capabilities.push("thinking");
            }
            if model.capability.image_in {
                capabilities.push("image_in");
            }
            let capability_label = if capabilities.is_empty() {
                String::new()
            } else {
                format!(" [{}]", capabilities.join(","))
            };
            runtime.write_stdout(&format!(
                "  {}  ctx={}{}\n",
                model.id, model.capability.max_context_tokens, capability_label
            ));
        }
        return Ok(());
    }

    let filter = filter.map(str::to_lowercase);
    let mut entries: Vec<(&String, &CatalogProviderEntry)> = catalog
        .iter()
        .filter(|(id, entry)| {
            filter.as_ref().is_none_or(|filter| {
                format!("{} {}", id, entry.name.as_deref().unwrap_or_default())
                    .to_lowercase()
                    .contains(filter)
            })
        })
        .collect();
    entries.sort_by_key(|(id, _)| *id);

    if json {
        let output: BTreeMap<&str, &CatalogProviderEntry> = entries
            .iter()
            .map(|(id, entry)| (id.as_str(), *entry))
            .collect();
        let output = serde_json::to_string_pretty(&output).map_err(ProviderError::from)?;
        runtime.write_stdout(&format!("{output}\n"));
        return Ok(());
    }
    if entries.is_empty() {
        if let Some(filter) = filter {
            runtime.write_stdout(&format!("No providers in catalog match \"{filter}\".\n"));
        } else {
            runtime.write_stdout("Catalog is empty.\n");
        }
        return Ok(());
    }

    for (id, entry) in entries {
        let model_count = entry.models.as_ref().map_or(0, IndexMap::len);
        let wire = infer_wire_type(entry).map_or("?", WireType::as_str);
        runtime.write_stdout(&format!(
            "{id}  wire={wire}  models={model_count}  {}\n",
            entry.name.as_deref().unwrap_or_default()
        ));
    }
    Ok(())
}

// Original:
//   packages/kosong/src/catalog.ts
//   inferWireType()
pub fn infer_wire_type(entry: &CatalogProviderEntry) -> Option<WireType> {
    match entry.provider_type.as_deref() {
        Some("anthropic") => return Some(WireType::Anthropic),
        Some("openai") => return Some(WireType::OpenAi),
        Some("kimi") => return Some(WireType::Kimi),
        Some("google-genai") => return Some(WireType::GoogleGenAi),
        Some("openai_responses") => return Some(WireType::OpenAiResponses),
        Some("vertexai") => return Some(WireType::VertexAi),
        _ => {}
    }
    let npm = entry.npm.as_deref().unwrap_or_default().to_lowercase();
    let id = entry.id.as_deref().unwrap_or_default().to_lowercase();
    if npm.contains("anthropic") || id.contains("anthropic") || id.contains("claude") {
        Some(WireType::Anthropic)
    } else if id.contains("vertex") {
        Some(WireType::VertexAi)
    } else if npm.contains("google") || id.contains("google") || id.contains("gemini") {
        Some(WireType::GoogleGenAi)
    } else if npm.contains("openai") || id.contains("openai") {
        Some(WireType::OpenAi)
    } else {
        None
    }
}

// Original:
//   packages/kosong/src/catalog.ts
//   catalogProviderModels(), catalogModelToCapability()
pub fn catalog_provider_models(entry: &CatalogProviderEntry) -> Vec<CatalogModel> {
    entry
        .models
        .iter()
        .flat_map(|models| models.values())
        .filter_map(catalog_model_to_capability)
        .collect()
}

fn catalog_model_to_capability(model: &CatalogModelEntry) -> Option<CatalogModel> {
    let id = model.id.as_deref().filter(|id| !id.is_empty())?;
    let context = positive_integer(model.limit.as_ref()?.context?)?;
    if !is_usable_chat_model(model) {
        return None;
    }
    let inputs = model
        .modalities
        .as_ref()
        .and_then(|modalities| modalities.input.as_ref());
    Some(CatalogModel {
        id: id.to_owned(),
        name: model
            .name
            .as_deref()
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
        max_output_size: model
            .limit
            .as_ref()
            .and_then(|limit| limit.output)
            .and_then(positive_number),
        reasoning_key: catalog_reasoning_key(model.interleaved.as_ref()),
        capability: CatalogCapability {
            image_in: contains_modality(inputs, "image"),
            video_in: contains_modality(inputs, "video"),
            audio_in: contains_modality(inputs, "audio"),
            thinking: model.reasoning.unwrap_or(false),
            tool_use: model.tool_call.unwrap_or(true),
            max_context_tokens: context,
            dynamically_loaded_tools: model.dynamically_loaded_tools == Some(true),
        },
    })
}

fn positive_integer(value: f64) -> Option<u64> {
    (value.is_finite() && value > 0.0 && value.fract() == 0.0 && value <= u64::MAX as f64)
        .then_some(value as u64)
}

fn positive_number(value: f64) -> Option<f64> {
    (value.is_finite() && value > 0.0).then_some(value)
}

fn contains_modality(modalities: Option<&Vec<String>>, expected: &str) -> bool {
    modalities.is_some_and(|values| values.iter().any(|value| value == expected))
}

fn is_usable_chat_model(model: &CatalogModelEntry) -> bool {
    if model
        .modalities
        .as_ref()
        .and_then(|modalities| modalities.output.as_ref())
        .is_some_and(|outputs| !outputs.iter().any(|output| output == "text"))
    {
        return false;
    }
    ![
        model.family.as_deref(),
        model.id.as_deref(),
        model.name.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(has_embedding_marker)
}

fn has_embedding_marker(value: &str) -> bool {
    let lower = value.to_lowercase();
    lower.contains("embedding") || lower.split(['-', '_', '/']).any(|part| part == "embed")
}

fn catalog_reasoning_key(interleaved: Option<&Value>) -> Option<String> {
    match interleaved {
        Some(Value::Bool(true)) => Some("reasoning_content".to_owned()),
        Some(Value::Object(object)) => object
            .get("field")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|field| !field.is_empty())
            .map(str::to_owned),
        _ => None,
    }
}

// Original:
//   apps/kimi-code/src/cli/sub/provider.ts
//   handleProviderRemove()
pub async fn handle_provider_remove(
    runtime: &dyn ProviderRuntime,
    provider_id: &str,
) -> Result<(), ProviderCommandError> {
    runtime.ensure_config_file().await?;
    let config = runtime.get_config().await?;
    if !config.providers.contains_key(provider_id) {
        runtime.write_stderr(&format!("Provider \"{provider_id}\" not found.\n"));
        return Err(ProviderCommandError::Exit(1));
    }

    runtime.remove_provider(provider_id).await?;
    runtime.write_stdout(&format!("Removed provider \"{provider_id}\".\n"));
    Ok(())
}

// Original:
//   apps/kimi-code/src/cli/sub/provider.ts
//   handleProviderList()
pub async fn handle_provider_list(
    runtime: &dyn ProviderRuntime,
    json: bool,
) -> Result<(), ProviderCommandError> {
    runtime.ensure_config_file().await?;
    let config = runtime.get_config().await?;

    if json {
        #[derive(Serialize)]
        struct ProviderList<'a> {
            providers: &'a BTreeMap<String, ProviderDefinition>,
            models: &'a BTreeMap<String, ModelDefinition>,
        }

        let output = serde_json::to_string_pretty(&ProviderList {
            providers: &config.providers,
            models: &config.models,
        })
        .map_err(ProviderError::from)?;
        runtime.write_stdout(&format!("{output}\n"));
        return Ok(());
    }

    let mut models_by_provider: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (alias, model) in &config.models {
        models_by_provider
            .entry(&model.provider)
            .or_default()
            .push(alias);
    }

    if config.providers.is_empty() {
        runtime.write_stdout("No providers configured.\n");
        return Ok(());
    }

    for (id, provider) in &config.providers {
        let model_count = models_by_provider.get(id.as_str()).map_or(0, Vec::len);
        let source_label = provider_source_label(provider);
        runtime.write_stdout(&format!(
            "{id}  type={}  models={model_count}  source={source_label}\n",
            provider.provider_type
        ));
    }
    if let Some(default_model) = config.default_model {
        runtime.write_stdout(&format!("\nDefault model: {default_model}\n"));
    }

    Ok(())
}

// Original:
//   apps/kimi-code/src/cli/sub/provider.ts
//   resolveApiKey()
pub fn resolve_api_key(flag: Option<&str>, environment_value: Option<&str>) -> Option<String> {
    flag.filter(|value| !value.is_empty())
        .or_else(|| environment_value.filter(|value| !value.is_empty()))
        .map(str::to_owned)
}

// Original:
//   apps/kimi-code/src/cli/sub/provider.ts
//   providerSourceLabel()
pub fn provider_source_label(provider: &ProviderDefinition) -> String {
    if let Some(source) = &provider.source
        && source.get("kind").and_then(Value::as_str) == Some("apiJson")
        && let Some(url) = source.get("url").and_then(Value::as_str)
    {
        return format!("apiJson({url})");
    }
    if provider.oauth.is_some() {
        return "oauth".to_owned();
    }
    "inline".to_owned()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;

    struct RuntimeMock {
        config: Mutex<ProviderConfig>,
        registry_entries: Mutex<Vec<CustomRegistryProviderEntry>>,
        fetch_error: Mutex<Option<(String, Option<u16>)>>,
        catalog: Mutex<Catalog>,
        catalog_error: Mutex<Option<(String, Option<u16>)>>,
        catalog_urls: Mutex<Vec<String>>,
        get_error: bool,
        ensure_calls: Mutex<usize>,
        remove_calls: Mutex<Vec<String>>,
        set_calls: Mutex<Vec<ProviderConfigPatch>>,
        stdout: Mutex<String>,
        stderr: Mutex<String>,
    }

    impl RuntimeMock {
        fn new(config: ProviderConfig) -> Self {
            Self {
                config: Mutex::new(config),
                registry_entries: Mutex::new(Vec::new()),
                fetch_error: Mutex::new(None),
                catalog: Mutex::new(Catalog::new()),
                catalog_error: Mutex::new(None),
                catalog_urls: Mutex::new(Vec::new()),
                get_error: false,
                ensure_calls: Mutex::new(0),
                remove_calls: Mutex::new(Vec::new()),
                set_calls: Mutex::new(Vec::new()),
                stdout: Mutex::new(String::new()),
                stderr: Mutex::new(String::new()),
            }
        }
    }

    #[async_trait]
    impl ProviderRegistryRuntime for RuntimeMock {
        async fn fetch_custom_registry(
            &self,
            _: &CustomRegistrySource,
        ) -> Result<Vec<CustomRegistryProviderEntry>, RegistryFetchError> {
            if let Some((message, status)) = self.fetch_error.lock().expect("fetch error").take() {
                return Err(RegistryFetchError::new(
                    std::io::Error::other(message),
                    status,
                ));
            }
            Ok(self.registry_entries.lock().expect("entries").clone())
        }
    }

    #[async_trait]
    impl ProviderCatalogRuntime for RuntimeMock {
        async fn fetch_catalog(&self, url: &str) -> Result<Catalog, CatalogFetchError> {
            self.catalog_urls
                .lock()
                .expect("catalog URLs")
                .push(url.to_owned());
            if let Some((message, status)) =
                self.catalog_error.lock().expect("catalog error").take()
            {
                return Err(CatalogFetchError::new(
                    std::io::Error::other(message),
                    status,
                ));
            }
            Ok(self.catalog.lock().expect("catalog").clone())
        }
    }

    #[async_trait]
    impl ProviderRuntime for RuntimeMock {
        async fn ensure_config_file(&self) -> Result<(), ProviderError> {
            *self.ensure_calls.lock().expect("ensure calls") += 1;
            Ok(())
        }

        async fn get_config(&self) -> Result<ProviderConfig, ProviderError> {
            if self.get_error {
                return Err(ProviderError::new(std::io::Error::other(
                    "Cannot change settings while config.toml is invalid",
                )));
            }
            Ok(self.config.lock().expect("config").clone())
        }

        async fn remove_provider(
            &self,
            provider_id: &str,
        ) -> Result<ProviderConfig, ProviderError> {
            self.remove_calls
                .lock()
                .expect("remove calls")
                .push(provider_id.to_owned());
            let mut config = self.config.lock().expect("config");
            config.providers.remove(provider_id);
            let removed_default = config.default_model.as_ref().is_some_and(|default_model| {
                config
                    .models
                    .get(default_model)
                    .is_some_and(|model| model.provider == provider_id)
            });
            config
                .models
                .retain(|_, model| model.provider != provider_id);
            if removed_default {
                config.default_model = None;
            }
            Ok(config.clone())
        }

        async fn set_config(
            &self,
            patch: &ProviderConfigPatch,
        ) -> Result<ProviderConfig, ProviderError> {
            self.set_calls
                .lock()
                .expect("set calls")
                .push(patch.clone());
            let mut config = self.config.lock().expect("config");
            if let Some(providers) = &patch.providers {
                config.providers = providers.clone();
            }
            if let Some(models) = &patch.models {
                config.models = models.clone();
            }
            if let Some(default_model) = &patch.default_model {
                config.default_model = Some(default_model.clone());
            }
            if let Some(thinking) = &patch.thinking {
                config.thinking = Some(thinking.clone());
            }
            Ok(config.clone())
        }

        fn write_stdout(&self, text: &str) {
            self.stdout.lock().expect("stdout").push_str(text);
        }

        fn write_stderr(&self, text: &str) {
            self.stderr.lock().expect("stderr").push_str(text);
        }
    }

    fn definition(provider_type: &str) -> ProviderDefinition {
        ProviderDefinition {
            provider_type: provider_type.to_owned(),
            base_url: None,
            api_key: None,
            oauth: None,
            source: None,
            additional_fields: Map::new(),
        }
    }

    fn model(provider: &str, model: &str) -> ModelDefinition {
        ModelDefinition {
            provider: provider.to_owned(),
            model: model.to_owned(),
            additional_fields: Map::new(),
        }
    }

    fn registry_entry(id: &str, model_id: &str) -> CustomRegistryProviderEntry {
        CustomRegistryProviderEntry {
            id: id.to_owned(),
            name: format!("{id} display"),
            api: format!("https://{id}.example.test"),
            provider_type: "anthropic".to_owned(),
            env: None,
            models: BTreeMap::from([(
                model_id.to_owned(),
                CustomRegistryModelEntry {
                    id: model_id.to_owned(),
                    name: Some(format!("{model_id} display")),
                    limit: None,
                    tool_call: Some(true),
                    reasoning: None,
                    modalities: None,
                    support_efforts: None,
                    default_effort: None,
                },
            )]),
        }
    }

    fn catalog_model(id: &str, reasoning: bool, image_input: bool) -> CatalogModelEntry {
        CatalogModelEntry {
            id: Some(id.to_owned()),
            name: Some(format!("{id} display")),
            family: None,
            limit: Some(CatalogLimit {
                context: Some(200_000.0),
                output: Some(64_000.0),
            }),
            tool_call: Some(true),
            reasoning: Some(reasoning),
            dynamically_loaded_tools: None,
            interleaved: None,
            modalities: Some(CatalogModalities {
                input: Some(if image_input {
                    vec!["text".to_owned(), "image".to_owned()]
                } else {
                    vec!["text".to_owned()]
                }),
                output: Some(vec!["text".to_owned()]),
            }),
            additional_fields: Map::new(),
        }
    }

    fn catalog_fixture() -> Catalog {
        IndexMap::from([
            (
                "openai".to_owned(),
                CatalogProviderEntry {
                    id: Some("openai".to_owned()),
                    name: Some("OpenAI".to_owned()),
                    api: Some("https://api.openai.com/v1".to_owned()),
                    env: Some(vec!["OPENAI_API_KEY".to_owned()]),
                    npm: Some("@ai-sdk/openai".to_owned()),
                    provider_type: None,
                    models: Some(IndexMap::from([(
                        "gpt-5.5".to_owned(),
                        catalog_model("gpt-5.5", true, true),
                    )])),
                    additional_fields: Map::new(),
                },
            ),
            (
                "anthropic".to_owned(),
                CatalogProviderEntry {
                    id: Some("anthropic".to_owned()),
                    name: Some("Anthropic".to_owned()),
                    api: Some("https://api.anthropic.com/v1".to_owned()),
                    env: Some(vec!["ANTHROPIC_API_KEY".to_owned()]),
                    npm: Some("@ai-sdk/anthropic".to_owned()),
                    provider_type: None,
                    models: Some(IndexMap::from([
                        (
                            "claude-opus".to_owned(),
                            catalog_model("claude-opus", true, true),
                        ),
                        (
                            "claude-haiku".to_owned(),
                            catalog_model("claude-haiku", false, false),
                        ),
                    ])),
                    additional_fields: Map::new(),
                },
            ),
        ])
    }

    #[tokio::test]
    async fn command_dispatch_routes_provider_and_catalog_arguments() {
        let runtime = RuntimeMock::new(ProviderConfig::default());
        *runtime.registry_entries.lock().expect("entries") = vec![registry_entry("kohub", "model")];
        let code = run_provider_command(
            &runtime,
            &ProviderCommand::Add {
                url: "https://registry.test".to_owned(),
                api_key: None,
            },
            Some("environment-key"),
        )
        .await;
        assert_eq!(code, 0);
        assert_eq!(
            runtime.config.lock().expect("config").providers["kohub"]
                .api_key
                .as_deref(),
            Some("environment-key")
        );

        runtime.stdout.lock().expect("stdout").clear();
        *runtime.catalog.lock().expect("catalog") = catalog_fixture();
        let code = run_provider_command(
            &runtime,
            &ProviderCommand::Catalog {
                command: CatalogCommand::List {
                    provider_id: Some("openai".to_owned()),
                    filter: Some("ignored-for-provider-view".to_owned()),
                    url: Some("https://catalog.test".to_owned()),
                    json: false,
                },
            },
            None,
        )
        .await;
        assert_eq!(code, 0);
        assert!(
            runtime
                .stdout
                .lock()
                .expect("stdout")
                .starts_with("OpenAI (openai)")
        );
        assert_eq!(
            runtime
                .catalog_urls
                .lock()
                .expect("catalog URLs")
                .last()
                .map(String::as_str),
            Some("https://catalog.test")
        );
    }

    #[tokio::test]
    async fn command_dispatch_converts_expected_and_unexpected_failures_to_exit_one() {
        let runtime = RuntimeMock::new(ProviderConfig::default());
        let code = run_provider_command(
            &runtime,
            &ProviderCommand::Remove {
                provider_id: "missing".to_owned(),
            },
            None,
        )
        .await;
        assert_eq!(code, 1);
        assert_eq!(
            runtime.stderr.lock().expect("stderr").as_str(),
            "Provider \"missing\" not found.\n"
        );

        let mut runtime = RuntimeMock::new(ProviderConfig::default());
        runtime.get_error = true;
        let code =
            run_provider_command(&runtime, &ProviderCommand::List { json: false }, None).await;
        assert_eq!(code, 1);
        assert_eq!(
            runtime.stderr.lock().expect("stderr").as_str(),
            "Cannot change settings while config.toml is invalid\n"
        );
    }

    #[tokio::test]
    async fn catalog_add_imports_models_and_preserves_unrelated_defaults() {
        let mut config = ProviderConfig::default();
        config
            .providers
            .insert("other".to_owned(), definition("kimi"));
        config
            .models
            .insert("other/main".to_owned(), model("other", "main"));
        config.default_model = Some("other/main".to_owned());
        config.thinking = Some(json!({ "enabled": true, "effort": "high" }));
        let runtime = RuntimeMock::new(config);
        *runtime.catalog.lock().expect("catalog") = catalog_fixture();

        handle_catalog_add(&runtime, "anthropic", Some("sk-ant"), None, None, None)
            .await
            .expect("catalog add");

        let config = runtime.config.lock().expect("config");
        assert_eq!(config.providers["anthropic"].provider_type, "anthropic");
        assert_eq!(
            config.providers["anthropic"].base_url.as_deref(),
            Some("https://api.anthropic.com")
        );
        assert!(config.models.contains_key("anthropic/claude-opus"));
        assert!(config.models.contains_key("other/main"));
        assert_eq!(config.default_model.as_deref(), Some("other/main"));
        assert_eq!(
            config.thinking,
            Some(json!({ "enabled": true, "effort": "high" }))
        );
        drop(config);
        assert!(
            runtime
                .stdout
                .lock()
                .expect("stdout")
                .contains("Imported Anthropic (anthropic) with 2 models")
        );
    }

    #[tokio::test]
    async fn catalog_add_sets_requested_default_without_overwriting_thinking() {
        let mut config = ProviderConfig {
            thinking: Some(json!({ "enabled": true })),
            ..ProviderConfig::default()
        };
        config.additional_fields.insert("keep".to_owned(), json!(1));
        let runtime = RuntimeMock::new(config);
        *runtime.catalog.lock().expect("catalog") = catalog_fixture();

        handle_catalog_add(
            &runtime,
            "anthropic",
            None,
            Some("sk-env"),
            Some("claude-opus"),
            None,
        )
        .await
        .expect("catalog add");

        let config = runtime.config.lock().expect("config");
        assert_eq!(
            config.default_model.as_deref(),
            Some("anthropic/claude-opus")
        );
        assert_eq!(config.thinking, Some(json!({ "enabled": true })));
        assert_eq!(config.additional_fields["keep"], 1);
        drop(config);
        assert!(
            runtime
                .stdout
                .lock()
                .expect("stdout")
                .contains("Default model set to anthropic/claude-opus")
        );
    }

    #[tokio::test]
    async fn catalog_reimport_restores_resolvable_default_and_drops_stale_default() {
        for (default_model, expected) in [
            ("anthropic/claude-opus", Some("anthropic/claude-opus")),
            ("anthropic/legacy", None),
        ] {
            let mut config = ProviderConfig::default();
            config
                .providers
                .insert("anthropic".to_owned(), definition("anthropic"));
            let model_id = default_model.split('/').nth(1).expect("model id");
            config
                .models
                .insert(default_model.to_owned(), model("anthropic", model_id));
            config.default_model = Some(default_model.to_owned());
            let runtime = RuntimeMock::new(config);
            *runtime.catalog.lock().expect("catalog") = catalog_fixture();

            handle_catalog_add(&runtime, "anthropic", Some("rotated"), None, None, None)
                .await
                .expect("reimport");

            assert_eq!(
                runtime
                    .config
                    .lock()
                    .expect("config")
                    .default_model
                    .as_deref(),
                expected
            );
        }
    }

    #[tokio::test]
    async fn catalog_add_rejects_invalid_inputs_before_mutating_config() {
        let runtime = RuntimeMock::new(ProviderConfig::default());
        *runtime.catalog.lock().expect("catalog") = catalog_fixture();

        let missing_key = handle_catalog_add(&runtime, "anthropic", None, None, None, None)
            .await
            .expect_err("missing key");
        assert!(matches!(missing_key, ProviderCommandError::Exit(1)));
        assert!(
            runtime
                .catalog_urls
                .lock()
                .expect("catalog URLs")
                .is_empty()
        );

        let invalid_model = handle_catalog_add(
            &runtime,
            "anthropic",
            Some("key"),
            None,
            Some("unknown"),
            None,
        )
        .await
        .expect_err("unknown model");
        assert!(matches!(invalid_model, ProviderCommandError::Exit(1)));
        assert!(runtime.set_calls.lock().expect("set calls").is_empty());
        assert!(
            runtime
                .stderr
                .lock()
                .expect("stderr")
                .contains("kimi provider catalog list anthropic")
        );
    }

    #[test]
    fn catalog_base_url_only_strips_anthropic_v1_suffix() {
        let mut provider = catalog_fixture()
            .shift_remove("anthropic")
            .expect("provider");
        assert_eq!(
            catalog_base_url(&provider, WireType::Anthropic).as_deref(),
            Some("https://api.anthropic.com")
        );
        assert_eq!(
            catalog_base_url(&provider, WireType::OpenAi).as_deref(),
            Some("https://api.anthropic.com/v1")
        );
        provider.api = Some("".to_owned());
        assert_eq!(catalog_base_url(&provider, WireType::Anthropic), None);
    }

    #[tokio::test]
    async fn catalog_list_sorts_providers_and_filters_case_insensitively() {
        let runtime = RuntimeMock::new(ProviderConfig::default());
        *runtime.catalog.lock().expect("catalog") = catalog_fixture();

        handle_catalog_list(&runtime, None, false, None, None)
            .await
            .expect("catalog list");

        let output = runtime.stdout.lock().expect("stdout").clone();
        assert!(output.starts_with("anthropic  wire=anthropic  models=2  Anthropic\n"));
        assert!(output.contains("openai  wire=openai  models=1  OpenAI"));
        drop(output);
        runtime.stdout.lock().expect("stdout").clear();

        handle_catalog_list(&runtime, None, false, Some("OPEN"), None)
            .await
            .expect("filtered list");
        let output = runtime.stdout.lock().expect("stdout");
        assert!(output.contains("openai"));
        assert!(!output.contains("anthropic"));
    }

    #[tokio::test]
    async fn catalog_provider_view_lists_models_and_capabilities_in_source_order() {
        let runtime = RuntimeMock::new(ProviderConfig::default());
        *runtime.catalog.lock().expect("catalog") = catalog_fixture();

        handle_catalog_list(&runtime, Some("anthropic"), false, None, None)
            .await
            .expect("provider models");

        let output = runtime.stdout.lock().expect("stdout");
        assert!(output.starts_with("Anthropic (anthropic)\n"));
        assert!(output.contains("claude-opus  ctx=200000 [tool_use,thinking,image_in]"));
        assert!(output.contains("claude-haiku  ctx=200000 [tool_use]"));
        assert!(
            output.find("claude-opus").expect("opus") < output.find("claude-haiku").expect("haiku")
        );
    }

    #[tokio::test]
    async fn catalog_json_normalizes_models_and_honors_url_override() {
        let runtime = RuntimeMock::new(ProviderConfig::default());
        *runtime.catalog.lock().expect("catalog") = catalog_fixture();

        handle_catalog_list(
            &runtime,
            Some("openai"),
            true,
            None,
            Some("https://example.test/catalog.json"),
        )
        .await
        .expect("catalog JSON");

        let output: Value =
            serde_json::from_str(&runtime.stdout.lock().expect("stdout")).expect("valid JSON");
        assert_eq!(output["providerId"], "openai");
        assert_eq!(output["models"][0]["id"], "gpt-5.5");
        assert_eq!(
            output["models"][0]["capability"]["max_context_tokens"],
            200_000
        );
        assert_eq!(
            runtime
                .catalog_urls
                .lock()
                .expect("catalog URLs")
                .as_slice(),
            ["https://example.test/catalog.json"]
        );
    }

    #[tokio::test]
    async fn catalog_list_handles_missing_provider_fetch_error_and_empty_filter() {
        let runtime = RuntimeMock::new(ProviderConfig::default());
        *runtime.catalog.lock().expect("catalog") = catalog_fixture();
        let missing = handle_catalog_list(&runtime, Some("unknown"), false, None, None)
            .await
            .expect_err("missing provider");
        assert!(matches!(missing, ProviderCommandError::Exit(1)));

        *runtime.catalog_error.lock().expect("catalog error") =
            Some(("service unavailable".to_owned(), Some(503)));
        let fetch = handle_catalog_list(&runtime, None, false, None, None)
            .await
            .expect_err("fetch error");
        assert!(matches!(fetch, ProviderCommandError::Exit(1)));
        assert!(runtime.stderr.lock().expect("stderr").contains("HTTP 503"));

        runtime.stdout.lock().expect("stdout").clear();
        *runtime.catalog.lock().expect("catalog") = catalog_fixture();
        handle_catalog_list(&runtime, None, false, Some("missing"), None)
            .await
            .expect("empty filter");
        assert_eq!(
            runtime.stdout.lock().expect("stdout").as_str(),
            "No providers in catalog match \"missing\".\n"
        );
    }

    #[test]
    fn catalog_normalization_skips_non_chat_models_and_preserves_reasoning_key() {
        let mut provider = catalog_fixture().swap_remove("openai").expect("provider");
        let models = provider.models.as_mut().expect("models");
        let mut embedding = catalog_model("text-embedding-3", false, false);
        embedding.family = Some("embedding".to_owned());
        models.insert("embedding".to_owned(), embedding);
        let mut audio_only = catalog_model("audio", false, false);
        audio_only.modalities.as_mut().expect("modalities").output = Some(vec!["audio".to_owned()]);
        models.insert("audio".to_owned(), audio_only);
        models.get_mut("gpt-5.5").expect("GPT").interleaved =
            Some(json!({ "field": " reasoning " }));

        let normalized = catalog_provider_models(&provider);

        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].id, "gpt-5.5");
        assert_eq!(normalized[0].reasoning_key.as_deref(), Some("reasoning"));
    }

    #[tokio::test]
    async fn imports_all_custom_registry_providers_in_one_config_write() {
        let runtime = RuntimeMock::new(ProviderConfig::default());
        *runtime.registry_entries.lock().expect("entries") = vec![
            registry_entry("kohub", "claude-opus"),
            registry_entry("kohub-responses", "gpt-5"),
        ];

        handle_provider_add(
            &runtime,
            " https://registry.example.test/api.json ",
            Some("sk-test"),
            None,
        )
        .await
        .expect("add providers");

        let config = runtime.config.lock().expect("config");
        assert_eq!(config.providers.len(), 2);
        assert_eq!(
            config.providers["kohub"].base_url.as_deref(),
            Some("https://kohub.example.test")
        );
        assert_eq!(
            config.providers["kohub"].api_key.as_deref(),
            Some("sk-test")
        );
        assert_eq!(
            config.providers["kohub"].source.as_ref().expect("source")["url"],
            "https://registry.example.test/api.json"
        );
        assert!(config.models.contains_key("kohub/claude-opus"));
        assert!(config.models.contains_key("kohub-responses/gpt-5"));
        drop(config);
        assert_eq!(runtime.set_calls.lock().expect("set calls").len(), 1);
        let output = runtime.stdout.lock().expect("stdout");
        assert!(output.contains("Imported 2 providers (2 models)"));
        assert!(output.contains("  - kohub\n"));
    }

    #[tokio::test]
    async fn removes_every_stale_id_before_applying_registry_entries() {
        let mut config = ProviderConfig::default();
        config
            .providers
            .insert("second".to_owned(), definition("openai"));
        config
            .models
            .insert("second/old".to_owned(), model("second", "old"));
        let runtime = RuntimeMock::new(config);
        *runtime.registry_entries.lock().expect("entries") = vec![
            registry_entry("first", "new-a"),
            registry_entry("second", "new-b"),
        ];

        handle_provider_add(&runtime, "https://registry.test", Some("key"), None)
            .await
            .expect("replace provider");

        assert_eq!(
            runtime
                .remove_calls
                .lock()
                .expect("remove calls")
                .as_slice(),
            ["second"]
        );
        let config = runtime.config.lock().expect("config");
        assert!(config.providers.contains_key("first"));
        assert!(config.providers.contains_key("second"));
        assert!(!config.models.contains_key("second/old"));
        assert!(config.models.contains_key("first/new-a"));
        assert!(config.models.contains_key("second/new-b"));
    }

    #[tokio::test]
    async fn add_validates_api_key_url_fetch_and_empty_registry() {
        let runtime = RuntimeMock::new(ProviderConfig::default());
        let missing_key = handle_provider_add(&runtime, "https://registry.test", None, None)
            .await
            .expect_err("missing API key");
        assert!(matches!(missing_key, ProviderCommandError::Exit(1)));
        assert_eq!(*runtime.ensure_calls.lock().expect("ensure calls"), 0);

        let empty_url = handle_provider_add(&runtime, "  ", Some("key"), None)
            .await
            .expect_err("empty URL");
        assert!(matches!(empty_url, ProviderCommandError::Exit(1)));

        *runtime.fetch_error.lock().expect("fetch error") =
            Some(("invalid token".to_owned(), Some(401)));
        let fetch = handle_provider_add(&runtime, "https://registry.test", Some("bad"), None)
            .await
            .expect_err("fetch failure");
        assert!(matches!(fetch, ProviderCommandError::Exit(1)));
        assert!(runtime.stderr.lock().expect("stderr").contains("HTTP 401"));

        let empty = handle_provider_add(&runtime, "https://registry.test", Some("key"), None)
            .await
            .expect_err("empty registry");
        assert!(matches!(empty, ProviderCommandError::Exit(1)));
        assert!(
            runtime
                .stderr
                .lock()
                .expect("stderr")
                .contains("contained no usable providers")
        );
    }

    #[test]
    fn custom_registry_apply_preserves_user_fields_and_replaces_managed_fields() {
        let mut config = ProviderConfig::default();
        let mut old = model("kohub", "same");
        old.additional_fields
            .insert("custom".to_owned(), json!(true));
        old.additional_fields
            .insert("displayName".to_owned(), json!("old"));
        old.additional_fields
            .insert("supportEfforts".to_owned(), json!(["old"]));
        config.models.insert("kohub/same".to_owned(), old);
        config
            .models
            .insert("kohub/removed".to_owned(), model("kohub", "removed"));
        let mut entry = registry_entry("kohub", "same");
        let remote = entry.models.get_mut("same").expect("model");
        remote.limit = Some(CustomRegistryLimit {
            context: Some(200_000),
            output: Some(64_000),
        });
        remote.reasoning = Some(true);
        remote.modalities = Some(CustomRegistryModalities {
            input: Some(vec!["text".to_owned(), "image".to_owned()]),
            output: Some(vec!["audio".to_owned()]),
        });
        let source = CustomRegistrySource {
            kind: "apiJson".to_owned(),
            url: "https://registry.test".to_owned(),
            api_key: "key".to_owned(),
        };

        apply_custom_registry_provider(&mut config, &entry, &source);

        let model = &config.models["kohub/same"];
        assert_eq!(model.additional_fields["custom"], true);
        assert_eq!(model.additional_fields["displayName"], "same display");
        assert_eq!(model.additional_fields["maxContextSize"], 200_000);
        assert_eq!(
            model.additional_fields["capabilities"],
            json!(["tool_use", "thinking", "image_in", "audio_out"])
        );
        assert!(!model.additional_fields.contains_key("supportEfforts"));
        assert!(!config.models.contains_key("kohub/removed"));
    }

    #[tokio::test]
    async fn removes_existing_provider_and_reports_success() {
        let mut config = ProviderConfig::default();
        config
            .providers
            .insert("kohub".to_owned(), definition("anthropic"));
        config
            .models
            .insert("kohub/m".to_owned(), model("kohub", "m"));
        let runtime = RuntimeMock::new(config);

        handle_provider_remove(&runtime, "kohub")
            .await
            .expect("remove provider");

        assert_eq!(
            runtime
                .remove_calls
                .lock()
                .expect("remove calls")
                .as_slice(),
            ["kohub"]
        );
        assert!(
            !runtime
                .config
                .lock()
                .expect("config")
                .providers
                .contains_key("kohub")
        );
        assert!(
            runtime
                .stdout
                .lock()
                .expect("stdout")
                .contains("Removed provider \"kohub\"")
        );
    }

    #[tokio::test]
    async fn missing_provider_writes_error_and_requests_exit_one() {
        let runtime = RuntimeMock::new(ProviderConfig::default());

        let error = handle_provider_remove(&runtime, "nope")
            .await
            .expect_err("missing provider");

        assert!(matches!(error, ProviderCommandError::Exit(1)));
        assert!(
            runtime
                .stderr
                .lock()
                .expect("stderr")
                .contains("Provider \"nope\" not found")
        );
        assert!(
            runtime
                .remove_calls
                .lock()
                .expect("remove calls")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn lists_sorted_providers_with_model_counts_and_sources() {
        let mut config = ProviderConfig::default();
        let mut api_json = definition("anthropic");
        api_json.source = Some(Map::from_iter([
            ("kind".to_owned(), json!("apiJson")),
            ("url".to_owned(), json!("https://registry.example/api.json")),
        ]));
        let mut oauth = definition("kimi");
        oauth.oauth = Some(json!({ "storage": "file" }));
        config.providers.insert("kohub".to_owned(), api_json);
        config
            .providers
            .insert("managed:kimi-code".to_owned(), oauth);
        config
            .providers
            .insert("manual".to_owned(), definition("openai"));
        config
            .models
            .insert("kohub/a".to_owned(), model("kohub", "a"));
        config
            .models
            .insert("kohub/b".to_owned(), model("kohub", "b"));
        config
            .models
            .insert("manual/x".to_owned(), model("manual", "x"));
        config.default_model = Some("kohub/a".to_owned());
        let runtime = RuntimeMock::new(config);

        handle_provider_list(&runtime, false)
            .await
            .expect("list providers");

        let output = runtime.stdout.lock().expect("stdout");
        assert!(output.contains("kohub  type=anthropic  models=2  source=apiJson("));
        assert!(output.contains("managed:kimi-code  type=kimi  models=0  source=oauth"));
        assert!(output.contains("manual  type=openai  models=1  source=inline"));
        assert!(output.contains("Default model: kohub/a"));
        assert!(output.find("kohub").expect("kohub") < output.find("manual").expect("manual"));
    }

    #[tokio::test]
    async fn empty_list_has_friendly_message() {
        let runtime = RuntimeMock::new(ProviderConfig::default());

        handle_provider_list(&runtime, false)
            .await
            .expect("list providers");

        assert_eq!(
            runtime.stdout.lock().expect("stdout").as_str(),
            "No providers configured.\n"
        );
    }

    #[tokio::test]
    async fn json_list_preserves_external_field_names() {
        let mut config = ProviderConfig::default();
        let mut provider = definition("anthropic");
        provider.base_url = Some("https://example.test".to_owned());
        config.providers.insert("kohub".to_owned(), provider);
        let mut model = model("kohub", "a");
        model
            .additional_fields
            .insert("maxContextSize".to_owned(), json!(1024));
        config.models.insert("kohub/a".to_owned(), model);
        let runtime = RuntimeMock::new(config);

        handle_provider_list(&runtime, true)
            .await
            .expect("JSON list");

        let output: Value =
            serde_json::from_str(&runtime.stdout.lock().expect("stdout")).expect("valid JSON");
        assert_eq!(
            output["providers"]["kohub"]["baseUrl"],
            "https://example.test"
        );
        assert_eq!(output["models"]["kohub/a"]["maxContextSize"], 1024);
    }

    #[test]
    fn resolves_flag_before_environment_without_trimming() {
        assert_eq!(
            resolve_api_key(Some("flag"), Some("env")).as_deref(),
            Some("flag")
        );
        assert_eq!(
            resolve_api_key(Some(""), Some("env")).as_deref(),
            Some("env")
        );
        assert_eq!(resolve_api_key(Some(" "), None).as_deref(), Some(" "));
        assert_eq!(resolve_api_key(None, Some("")), None);
    }
}
