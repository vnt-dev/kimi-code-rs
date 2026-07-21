use std::{collections::BTreeMap, error::Error, fmt};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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

#[async_trait]
pub trait ProviderRuntime: Send + Sync {
    async fn ensure_config_file(&self) -> Result<(), ProviderError>;
    async fn get_config(&self) -> Result<ProviderConfig, ProviderError>;
    async fn remove_provider(&self, provider_id: &str) -> Result<ProviderConfig, ProviderError>;
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
    async fn set_config(&self, config: &ProviderConfig) -> Result<ProviderConfig, ProviderError>;
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

    runtime.set_config(&config).await?;
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
        ensure_calls: Mutex<usize>,
        remove_calls: Mutex<Vec<String>>,
        set_calls: Mutex<Vec<ProviderConfig>>,
        stdout: Mutex<String>,
        stderr: Mutex<String>,
    }

    impl RuntimeMock {
        fn new(config: ProviderConfig) -> Self {
            Self {
                config: Mutex::new(config),
                registry_entries: Mutex::new(Vec::new()),
                fetch_error: Mutex::new(None),
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

        async fn set_config(
            &self,
            config: &ProviderConfig,
        ) -> Result<ProviderConfig, ProviderError> {
            self.set_calls
                .lock()
                .expect("set calls")
                .push(config.clone());
            *self.config.lock().expect("config") = config.clone();
            Ok(config.clone())
        }
    }

    #[async_trait]
    impl ProviderRuntime for RuntimeMock {
        async fn ensure_config_file(&self) -> Result<(), ProviderError> {
            *self.ensure_calls.lock().expect("ensure calls") += 1;
            Ok(())
        }

        async fn get_config(&self) -> Result<ProviderConfig, ProviderError> {
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
            config
                .models
                .retain(|_, model| model.provider != provider_id);
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
