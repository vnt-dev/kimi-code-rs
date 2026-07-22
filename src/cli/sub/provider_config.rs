use std::{
    error::Error,
    fmt,
    io::Write,
    path::{Path, PathBuf},
};

use atomic_write_file::AtomicWriteFile;
use serde_json::{Map, Value};

use super::provider::{
    ModelDefinition, ProviderConfig, ProviderConfigPatch, ProviderDefinition, ProviderError,
};

const DEFAULT_CONFIG_FILE_TEXT: &str = "# ~/.kimi-code/config.toml\n\
# Runtime settings for Kimi Code.\n\
# This file starts empty so built-in defaults can apply.\n\
# Login will populate managed Kimi provider and model entries.\n";

#[derive(Debug)]
struct ConfigStoreError(String);

impl fmt::Display for ConfigStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ConfigStoreError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfigStore {
    config_path: PathBuf,
}

impl ProviderConfigStore {
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: config_path.into(),
        }
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    // Original:
    //   packages/agent-core/src/config/toml.ts
    //   ensureConfigFile()
    pub async fn ensure_config_file(&self) -> Result<(), ProviderError> {
        let path = self.config_path.clone();
        tokio::task::spawn_blocking(move || -> Result<(), std::io::Error> {
            let parent = path.parent().unwrap_or_else(|| Path::new("."));
            std::fs::create_dir_all(parent)?;
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => file.write_all(DEFAULT_CONFIG_FILE_TEXT.as_bytes()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                Err(error) => Err(error),
            }
        })
        .await
        .map_err(ProviderError::new)?
        .map_err(ProviderError::new)
    }

    // Original:
    //   packages/agent-core/src/config/toml.ts
    //   readConfigFileForUpdate()
    pub async fn get_config(&self) -> Result<ProviderConfig, ProviderError> {
        let path = self.config_path.clone();
        tokio::task::spawn_blocking(move || read_config(&path, false))
            .await
            .map_err(ProviderError::new)?
    }

    // Original:
    //   packages/agent-core/src/rpc/core-impl.ts
    //   setKimiConfig()
    pub async fn set_config(
        &self,
        patch: &ProviderConfigPatch,
    ) -> Result<ProviderConfig, ProviderError> {
        let path = self.config_path.clone();
        let patch = patch.clone();
        tokio::task::spawn_blocking(move || {
            let mut root = read_table_for_update(&path)?;
            apply_patch(&mut root, &patch)?;
            write_table(&path, &root)?;
            provider_config_from_table(&root, &path)
        })
        .await
        .map_err(ProviderError::new)?
    }

    // Original:
    //   packages/agent-core/src/rpc/core-impl.ts
    //   removeKimiProvider()
    pub async fn remove_provider(
        &self,
        provider_id: &str,
    ) -> Result<ProviderConfig, ProviderError> {
        let path = self.config_path.clone();
        let provider_id = provider_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut root = read_table_for_update(&path)?;
            if let Some(providers) = root
                .get_mut("providers")
                .and_then(toml::Value::as_table_mut)
            {
                providers.remove(&provider_id);
                if providers.is_empty() {
                    root.remove("providers");
                }
            }

            let default_model = root
                .get("default_model")
                .and_then(toml::Value::as_str)
                .map(str::to_owned);
            let mut removed_default = false;
            if let Some(models) = root.get_mut("models").and_then(toml::Value::as_table_mut) {
                models.retain(|alias, model| {
                    let remove = model
                        .as_table()
                        .and_then(|model| model.get("provider"))
                        .and_then(toml::Value::as_str)
                        == Some(provider_id.as_str());
                    if remove && default_model.as_deref() == Some(alias) {
                        removed_default = true;
                    }
                    !remove
                });
                if models.is_empty() {
                    root.remove("models");
                }
            }
            if removed_default {
                root.remove("default_model");
            }
            if root.get("default_provider").and_then(toml::Value::as_str)
                == Some(provider_id.as_str())
            {
                root.remove("default_provider");
            }

            write_table(&path, &root)?;
            provider_config_from_table(&root, &path)
        })
        .await
        .map_err(ProviderError::new)?
    }
}

fn read_config(path: &Path, for_update: bool) -> Result<ProviderConfig, ProviderError> {
    let table = if path.exists() {
        let text = std::fs::read_to_string(path).map_err(ProviderError::new)?;
        parse_table(&text, path, for_update)?
    } else {
        toml::Table::new()
    };
    provider_config_from_table(&table, path)
}

// Original:
//   packages/agent-core/src/config/rpc.ts
//   KimiConfigRpc.validateConfigToml()
//
// Rust adaptation:
//   Validation consumes the caller-provided snapshot instead of rereading the
//   file, so `kimi doctor` reports on exactly the bytes it loaded.
pub fn validate_provider_config_toml(text: &str, path: &Path) -> Result<(), ProviderError> {
    let table = parse_table(text, path, false)?;
    provider_config_from_table(&table, path).map(drop)
}

fn read_table_for_update(path: &Path) -> Result<toml::Table, ProviderError> {
    if !path.exists() {
        return Ok(toml::Table::new());
    }
    let text = std::fs::read_to_string(path).map_err(ProviderError::new)?;
    let table = parse_table(&text, path, true)?;
    provider_config_from_table(&table, path)?;
    Ok(table)
}

fn parse_table(text: &str, path: &Path, for_update: bool) -> Result<toml::Table, ProviderError> {
    if text.trim().is_empty() {
        return Ok(toml::Table::new());
    }
    toml::from_str(text).map_err(|error| {
        let message = if for_update {
            format!(
                "Cannot change settings while {} is invalid — fix it first (run `kimi doctor` for details).",
                path.display()
            )
        } else {
            format!("Invalid TOML in {}: {error}", path.display())
        };
        ProviderError::new(ConfigStoreError(message))
    })
}

fn provider_config_from_table(
    root: &toml::Table,
    path: &Path,
) -> Result<ProviderConfig, ProviderError> {
    let mut config = ProviderConfig::default();
    if let Some(providers) = root.get("providers") {
        let providers = providers.as_table().ok_or_else(|| {
            invalid_config(path, "providers must be a table keyed by provider id")
        })?;
        for (id, provider) in providers {
            let object = transformed_object(provider, TransformKind::Provider)
                .ok_or_else(|| invalid_config(path, &format!("providers.{id} must be a table")))?;
            let provider: ProviderDefinition = serde_json::from_value(Value::Object(object))
                .map_err(|error| invalid_config(path, &format!("providers.{id}: {error}")))?;
            config.providers.insert(id.clone(), provider);
        }
    }
    if let Some(models) = root.get("models") {
        let models = models
            .as_table()
            .ok_or_else(|| invalid_config(path, "models must be a table keyed by alias"))?;
        for (alias, model) in models {
            let object = transformed_object(model, TransformKind::Model)
                .ok_or_else(|| invalid_config(path, &format!("models.{alias} must be a table")))?;
            let model: ModelDefinition = serde_json::from_value(Value::Object(object))
                .map_err(|error| invalid_config(path, &format!("models.{alias}: {error}")))?;
            config.models.insert(alias.clone(), model);
        }
    }
    config.default_model = root
        .get("default_model")
        .and_then(toml::Value::as_str)
        .map(str::to_owned);
    if let Some(thinking) = root.get("thinking") {
        config.thinking = Some(transformed_value(thinking, true)?);
    }
    for (key, value) in root {
        if matches!(
            key.as_str(),
            "providers" | "models" | "default_model" | "thinking"
        ) {
            continue;
        }
        config
            .additional_fields
            .insert(snake_to_camel(key), transformed_value(value, false)?);
    }
    Ok(config)
}

#[derive(Clone, Copy)]
enum TransformKind {
    Provider,
    Model,
}

fn transformed_object(value: &toml::Value, kind: TransformKind) -> Option<Map<String, Value>> {
    let table = value.as_table()?;
    let mut output = Map::new();
    for (key, value) in table {
        let target_key = snake_to_camel(key);
        let transform_nested = matches!(
            (kind, target_key.as_str()),
            (TransformKind::Provider, "oauth") | (TransformKind::Model, "overrides")
        );
        let value = transformed_value(value, transform_nested).ok()?;
        output.insert(target_key, value);
    }
    Some(output)
}

fn transformed_value(value: &toml::Value, transform_keys: bool) -> Result<Value, ProviderError> {
    match value {
        toml::Value::Table(table) => {
            let mut object = Map::new();
            for (key, value) in table {
                object.insert(
                    if transform_keys {
                        snake_to_camel(key)
                    } else {
                        key.clone()
                    },
                    transformed_value(value, false)?,
                );
            }
            Ok(Value::Object(object))
        }
        _ => serde_json::to_value(value).map_err(ProviderError::new),
    }
}

fn apply_patch(root: &mut toml::Table, patch: &ProviderConfigPatch) -> Result<(), ProviderError> {
    if let Some(providers) = &patch.providers {
        if providers.is_empty() {
            root.remove("providers");
        } else {
            let mut table = toml::Table::new();
            for (id, provider) in providers {
                table.insert(
                    id.clone(),
                    definition_to_toml(provider, TransformKind::Provider)?,
                );
            }
            root.insert("providers".to_owned(), toml::Value::Table(table));
        }
    }
    if let Some(models) = &patch.models {
        if models.is_empty() {
            root.remove("models");
        } else {
            let mut table = toml::Table::new();
            for (alias, model) in models {
                table.insert(
                    alias.clone(),
                    definition_to_toml(model, TransformKind::Model)?,
                );
            }
            root.insert("models".to_owned(), toml::Value::Table(table));
        }
    }
    if let Some(default_model) = &patch.default_model {
        root.insert(
            "default_model".to_owned(),
            toml::Value::String(default_model.clone()),
        );
    }
    if let Some(thinking) = &patch.thinking {
        root.insert("thinking".to_owned(), json_to_toml(thinking, true)?);
    }
    Ok(())
}

fn definition_to_toml<T: serde::Serialize>(
    definition: &T,
    kind: TransformKind,
) -> Result<toml::Value, ProviderError> {
    let value = serde_json::to_value(definition).map_err(ProviderError::new)?;
    let Value::Object(object) = value else {
        return Err(ProviderError::new(ConfigStoreError(
            "provider configuration entry did not serialize as an object".to_owned(),
        )));
    };
    let mut table = toml::Table::new();
    for (key, value) in object {
        let transform_nested = matches!(
            (kind, key.as_str()),
            (TransformKind::Provider, "oauth") | (TransformKind::Model, "overrides")
        );
        table.insert(
            camel_to_snake(&key),
            json_to_toml(&value, transform_nested)?,
        );
    }
    Ok(toml::Value::Table(table))
}

fn json_to_toml(value: &Value, transform_keys: bool) -> Result<toml::Value, ProviderError> {
    match value {
        Value::Object(object) => {
            let mut table = toml::Table::new();
            for (key, value) in object {
                table.insert(
                    if transform_keys {
                        camel_to_snake(key)
                    } else {
                        key.clone()
                    },
                    json_to_toml(value, false)?,
                );
            }
            Ok(toml::Value::Table(table))
        }
        Value::Null => Err(ProviderError::new(ConfigStoreError(
            "TOML configuration cannot contain null values".to_owned(),
        ))),
        _ => toml::Value::try_from(value).map_err(ProviderError::new),
    }
}

fn write_table(path: &Path, root: &toml::Table) -> Result<(), ProviderError> {
    let content = format!("{}\n", toml::to_string(root).map_err(ProviderError::new)?);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(ProviderError::new)?;
    let mut file = AtomicWriteFile::open(path).map_err(ProviderError::new)?;
    file.write_all(content.as_bytes())
        .map_err(ProviderError::new)?;
    file.commit().map_err(ProviderError::new)
}

fn invalid_config(path: &Path, reason: &str) -> ProviderError {
    ProviderError::new(ConfigStoreError(format!(
        "Cannot change settings while {} is invalid — fix it first (run `kimi doctor` for details): {reason}.",
        path.display()
    )))
}

fn snake_to_camel(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut uppercase_next = false;
    for character in value.chars() {
        if character == '_' {
            uppercase_next = true;
        } else if uppercase_next {
            output.extend(character.to_uppercase());
            uppercase_next = false;
        } else {
            output.push(character);
        }
    }
    output
}

fn camel_to_snake(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_uppercase() {
            output.push('_');
            output.push(character.to_ascii_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::json;

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp_config(name: &str) -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "kimi-code-rs-provider-config-{}-{id}-{name}",
            std::process::id()
        ))
    }

    fn provider(provider_type: &str) -> ProviderDefinition {
        ProviderDefinition {
            provider_type: provider_type.to_owned(),
            base_url: Some("https://example.test/v1".to_owned()),
            api_key: Some("secret".to_owned()),
            oauth: None,
            source: None,
            additional_fields: Map::new(),
        }
    }

    fn model(provider: &str, model: &str) -> ModelDefinition {
        ModelDefinition {
            provider: provider.to_owned(),
            model: model.to_owned(),
            additional_fields: Map::from_iter([
                ("maxContextSize".to_owned(), json!(200_000)),
                ("capabilities".to_owned(), json!(["tool_use"])),
            ]),
        }
    }

    #[tokio::test]
    async fn ensure_creates_default_once_without_overwriting() {
        let path = temp_config("ensure.toml");
        let store = ProviderConfigStore::new(&path);
        store.ensure_config_file().await.expect("ensure config");
        let first = std::fs::read_to_string(&path).expect("read default");
        assert!(first.contains("Runtime settings for Kimi Code"));
        std::fs::write(&path, "telemetry = false\n").expect("customize");

        store.ensure_config_file().await.expect("ensure existing");

        assert_eq!(
            std::fs::read_to_string(&path).expect("read custom"),
            "telemetry = false\n"
        );
        std::fs::remove_file(path).expect("cleanup");
    }

    #[tokio::test]
    async fn set_config_writes_snake_case_and_preserves_unrelated_sections() {
        let path = temp_config("set.toml");
        std::fs::write(
            &path,
            "telemetry = false\n\n[permission]\nrules = [\"shell(*)\"]\n",
        )
        .expect("fixture");
        let store = ProviderConfigStore::new(&path);
        let patch = ProviderConfigPatch {
            providers: Some(std::collections::BTreeMap::from([(
                "anthropic".to_owned(),
                provider("anthropic"),
            )])),
            models: Some(std::collections::BTreeMap::from([(
                "anthropic/claude".to_owned(),
                model("anthropic", "claude"),
            )])),
            default_model: Some("anthropic/claude".to_owned()),
            thinking: Some(json!({ "enabled": true, "effort": "high" })),
        };

        let config = store.set_config(&patch).await.expect("set config");

        assert_eq!(config.default_model.as_deref(), Some("anthropic/claude"));
        let text = std::fs::read_to_string(&path).expect("read config");
        assert!(text.contains("default_model = \"anthropic/claude\""));
        assert!(text.contains("base_url = \"https://example.test/v1\""));
        assert!(text.contains("max_context_size = 200000"));
        assert!(text.contains("telemetry = false"));
        assert!(text.contains("[permission]"));
        std::fs::remove_file(path).expect("cleanup");
    }

    #[tokio::test]
    async fn remove_provider_clears_models_and_both_defaults() {
        let path = temp_config("remove.toml");
        std::fs::write(
            &path,
            r#"default_provider = "anthropic"
default_model = "anthropic/claude"
telemetry = false

[providers.anthropic]
type = "anthropic"
api_key = "secret"

[models."anthropic/claude"]
provider = "anthropic"
model = "claude"
max_context_size = 200000

[providers.other]
type = "kimi"
api_key = "other"

[models."other/main"]
provider = "other"
model = "main"
max_context_size = 1024
"#,
        )
        .expect("fixture");
        let store = ProviderConfigStore::new(&path);

        let config = store.remove_provider("anthropic").await.expect("remove");

        assert!(!config.providers.contains_key("anthropic"));
        assert!(config.providers.contains_key("other"));
        assert!(!config.models.contains_key("anthropic/claude"));
        assert!(config.models.contains_key("other/main"));
        assert_eq!(config.default_model, None);
        let text = std::fs::read_to_string(&path).expect("read config");
        assert!(!text.contains("default_provider"));
        assert!(text.contains("telemetry = false"));
        std::fs::remove_file(path).expect("cleanup");
    }

    #[tokio::test]
    async fn invalid_config_is_never_rewritten_by_update_paths() {
        let path = temp_config("invalid.toml");
        let invalid = "[providers.broken\ntype = 1\n";
        std::fs::write(&path, invalid).expect("fixture");
        let store = ProviderConfigStore::new(&path);

        let error = store
            .set_config(&ProviderConfigPatch::default())
            .await
            .expect_err("invalid config");

        assert!(error.to_string().contains("Cannot change settings while"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read invalid"),
            invalid
        );
        std::fs::remove_file(path).expect("cleanup");
    }

    #[tokio::test]
    async fn structurally_invalid_provider_is_rejected_before_rewrite() {
        let path = temp_config("invalid-provider.toml");
        let invalid = "[providers.broken]\ntype = 1\n";
        std::fs::write(&path, invalid).expect("fixture");
        let store = ProviderConfigStore::new(&path);

        let error = store
            .remove_provider("other")
            .await
            .expect_err("invalid provider");

        assert!(error.to_string().contains("providers.broken"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read invalid"),
            invalid
        );
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn validates_the_provided_config_snapshot_without_file_io() {
        let path = Path::new("/virtual/config.toml");
        validate_provider_config_toml(
            "telemetry = false\n\n[providers.example]\ntype = \"openai\"\n",
            path,
        )
        .expect("valid config");

        let error = validate_provider_config_toml("[providers.broken]\ntype = 1\n", path)
            .expect_err("invalid provider");
        assert!(error.to_string().contains("providers.broken"));
        assert!(error.to_string().contains("/virtual/config.toml"));
    }
}
