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
        ensure_calls: Mutex<usize>,
        remove_calls: Mutex<Vec<String>>,
        stdout: Mutex<String>,
        stderr: Mutex<String>,
    }

    impl RuntimeMock {
        fn new(config: ProviderConfig) -> Self {
            Self {
                config: Mutex::new(config),
                ensure_calls: Mutex::new(0),
                remove_calls: Mutex::new(Vec::new()),
                stdout: Mutex::new(String::new()),
                stderr: Mutex::new(String::new()),
            }
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
