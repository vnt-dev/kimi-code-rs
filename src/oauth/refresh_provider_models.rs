use std::{collections::HashSet, error::Error, fmt};

use async_trait::async_trait;
use indexmap::{IndexMap, IndexSet};
use serde_json::{Map, Value};

use super::{
    custom_registry::{
        CustomRegistryError, CustomRegistryProviderEntry, CustomRegistrySource,
        apply_custom_registry_provider, fetch_custom_registry, remove_custom_registry_provider,
    },
    managed_auth::{
        KIMI_CODE_PLATFORM_ID, KIMI_CODE_PROVIDER_NAME, ManagedKimiOAuthRef,
        ManagedKimiOAuthRefInput, OAuthStorageBackend, RuntimeAuthOptions,
        resolve_kimi_code_runtime_auth,
    },
    managed_config::{
        ManagedConfigError, ManagedKimiCodeApplyOptions, apply_managed_api_key_provider_models,
        apply_managed_kimi_code_config,
    },
    managed_models::{CredentialKind, ManagedModelsError, fetch_managed_kimi_code_models},
    managed_usage::is_managed_kimi_code_base_url,
    open_platform::{
        OpenPlatformError, apply_open_platform_config, fetch_open_platform_models,
        filter_models_by_prefix, get_open_platform_by_id, is_open_platform_id,
    },
};

#[derive(Debug)]
pub struct RefreshHostError(Box<dyn Error + Send + Sync>);

impl RefreshHostError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for RefreshHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for RefreshHostError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[async_trait]
pub trait RefreshProviderHost: Send + Sync {
    async fn get_config(&self) -> Result<Map<String, Value>, RefreshHostError>;
    async fn remove_provider(
        &self,
        provider_id: &str,
    ) -> Result<Map<String, Value>, RefreshHostError>;
    async fn set_config(
        &self,
        patch: Map<String, Value>,
    ) -> Result<Map<String, Value>, RefreshHostError>;
    async fn resolve_oauth_token(
        &self,
        provider_name: &str,
        oauth_ref: Option<&ManagedKimiOAuthRef>,
    ) -> Result<String, RefreshHostError>;
    fn user_agent(&self) -> Option<&str> {
        None
    }
}

#[derive(Debug)]
enum RefreshOperationError {
    Host(RefreshHostError),
    ManagedConfig(ManagedConfigError),
    ManagedModels(ManagedModelsError),
    OpenPlatform(OpenPlatformError),
    CustomRegistry(CustomRegistryError),
}

impl fmt::Display for RefreshOperationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Host(error) => error.fmt(formatter),
            Self::ManagedConfig(error) => error.fmt(formatter),
            Self::ManagedModels(error) => error.fmt(formatter),
            Self::OpenPlatform(error) => error.fmt(formatter),
            Self::CustomRegistry(error) => error.fmt(formatter),
        }
    }
}

impl Error for RefreshOperationError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderChange {
    pub provider_id: String,
    pub provider_name: String,
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RefreshResult {
    pub changed: Vec<ProviderChange>,
    pub unchanged: Vec<String>,
    pub failed: Vec<ProviderRefreshFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRefreshFailure {
    pub provider: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RefreshProviderScope {
    #[default]
    All,
    OAuth,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshProviderOptions {
    pub scope: RefreshProviderScope,
    pub provider_id: Option<String>,
}

// Original:
//   packages/oauth/src/refreshProviderModels.ts
//   refreshProviderModels()
pub async fn refresh_provider_models(
    host: &dyn RefreshProviderHost,
    options: &RefreshProviderOptions,
) -> Result<RefreshResult, RefreshHostError> {
    let mut result = RefreshResult::default();
    let target_id = options.provider_id.as_deref();
    let mut config = host.get_config().await?;

    let managed_provider = read_provider_object(&config, KIMI_CODE_PROVIDER_NAME).cloned();
    let managed_wanted = target_id.is_none_or(|target| target == KIMI_CODE_PROVIDER_NAME);
    if managed_wanted
        && managed_provider
            .as_ref()
            .is_some_and(|provider| provider.get("type").and_then(Value::as_str) == Some("kimi"))
        && managed_provider
            .as_ref()
            .and_then(|provider| provider.get("oauth"))
            .is_some()
    {
        let refresh = refresh_managed_oauth(host, &config, managed_provider.as_ref()).await;
        apply_branch_result(&mut config, &mut result, KIMI_CODE_PROVIDER_NAME, refresh);
    }

    if options.scope == RefreshProviderScope::OAuth {
        return Ok(result);
    }

    let open_platform_ids = provider_ids(&config)
        .into_iter()
        .filter(|provider_id| is_open_platform_id(provider_id))
        .collect::<Vec<_>>();
    for provider_id in open_platform_ids {
        if target_id.is_some_and(|target| target != provider_id) {
            continue;
        }
        let refresh = refresh_open_platform(host, &config, &provider_id).await;
        apply_branch_result(&mut config, &mut result, &provider_id, refresh);
    }

    let api_key_provider_ids = provider_ids(&config);
    for provider_id in api_key_provider_ids {
        if is_open_platform_id(&provider_id)
            || target_id.is_some_and(|target| target != provider_id)
        {
            continue;
        }
        let Some(provider) = read_provider_object(&config, &provider_id).cloned() else {
            continue;
        };
        if provider.get("type").and_then(Value::as_str) != Some("kimi")
            || provider.get("oauth").is_some()
            || read_custom_registry_source(&provider).is_some()
            || !is_managed_kimi_code_base_url(provider.get("baseUrl").and_then(Value::as_str))
            || resolve_provider_api_key(&provider).is_none()
        {
            continue;
        }
        let refresh = refresh_managed_api_key(host, &config, &provider_id, &provider).await;
        apply_branch_result(&mut config, &mut result, &provider_id, refresh);
    }

    let custom_groups = collect_custom_registry_groups(&config);
    for group in custom_groups.values() {
        if target_id.is_some_and(|target| !group.provider_ids.iter().any(|id| id == target)) {
            continue;
        }
        match refresh_custom_registry_group(host, &config, group, target_id).await {
            Ok(Some(group_result)) => {
                config = group_result.config;
                result.changed.extend(group_result.changed);
                result.unchanged.extend(group_result.unchanged);
            }
            Ok(None) => {}
            Err(error) => {
                let reported_ids = target_id.map_or_else(
                    || group.provider_ids.clone(),
                    |target| vec![target.to_owned()],
                );
                for provider_id in reported_ids {
                    result.failed.push(ProviderRefreshFailure {
                        provider: provider_id,
                        reason: error.to_string(),
                    });
                }
            }
        }
    }
    Ok(result)
}

enum ProviderBranchResult {
    Skipped,
    Unchanged(String),
    Changed {
        config: Map<String, Value>,
        change: ProviderChange,
    },
}

fn apply_branch_result(
    config: &mut Map<String, Value>,
    result: &mut RefreshResult,
    provider_id: &str,
    refresh: Result<ProviderBranchResult, RefreshOperationError>,
) {
    match refresh {
        Ok(ProviderBranchResult::Skipped) => {}
        Ok(ProviderBranchResult::Unchanged(provider_id)) => result.unchanged.push(provider_id),
        Ok(ProviderBranchResult::Changed {
            config: next,
            change,
        }) => {
            *config = next;
            result.changed.push(change);
        }
        Err(error) => result.failed.push(ProviderRefreshFailure {
            provider: provider_id.to_owned(),
            reason: error.to_string(),
        }),
    }
}

async fn refresh_managed_oauth(
    host: &dyn RefreshProviderHost,
    config: &Map<String, Value>,
    provider: Option<&Map<String, Value>>,
) -> Result<ProviderBranchResult, RefreshOperationError> {
    let Some(provider) = provider else {
        return Ok(ProviderBranchResult::Skipped);
    };
    let configured_oauth = provider.get("oauth").and_then(parse_oauth_ref_input);
    let environment = std::env::vars().collect();
    let auth = resolve_kimi_code_runtime_auth(RuntimeAuthOptions {
        configured_base_url: provider.get("baseUrl").and_then(Value::as_str),
        configured_oauth_ref: configured_oauth.as_ref(),
        environment: &environment,
    });
    let access_token = host
        .resolve_oauth_token(KIMI_CODE_PROVIDER_NAME, Some(&auth.oauth_ref))
        .await
        .map_err(RefreshOperationError::Host)?;
    let models = fetch_managed_kimi_code_models(
        &access_token,
        auth.base_url.as_deref(),
        None,
        CredentialKind::OAuth,
    )
    .await
    .map_err(RefreshOperationError::ManagedModels)?;
    if models.is_empty() {
        return Ok(ProviderBranchResult::Skipped);
    }
    let mut next = config.clone();
    apply_managed_kimi_code_config(
        &mut next,
        ManagedKimiCodeApplyOptions {
            models: &models,
            base_url: auth.base_url.as_deref(),
            oauth_key: Some(&auth.oauth_ref.key),
            oauth_host: auth.oauth_ref.oauth_host.as_deref(),
            preserve_default_model: true,
        },
    )
    .map_err(RefreshOperationError::ManagedConfig)?;
    prepare_refreshed_config(
        config,
        &mut next,
        KIMI_CODE_PROVIDER_NAME,
        &format!("{KIMI_CODE_PLATFORM_ID}/"),
    );
    finish_provider_refresh(
        host,
        config,
        next,
        KIMI_CODE_PROVIDER_NAME,
        "Kimi Code",
        &format!("{KIMI_CODE_PLATFORM_ID}/"),
        false,
    )
    .await
}

async fn refresh_open_platform(
    host: &dyn RefreshProviderHost,
    config: &Map<String, Value>,
    provider_id: &str,
) -> Result<ProviderBranchResult, RefreshOperationError> {
    let Some(platform) = get_open_platform_by_id(provider_id) else {
        return Ok(ProviderBranchResult::Skipped);
    };
    let Some(provider) = read_provider_object(config, provider_id) else {
        return Ok(ProviderBranchResult::Skipped);
    };
    let Some(api_key) = provider
        .get("apiKey")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty())
    else {
        return Ok(ProviderBranchResult::Skipped);
    };
    let fetched = fetch_open_platform_models(platform, api_key)
        .await
        .map_err(RefreshOperationError::OpenPlatform)?;
    let models = filter_models_by_prefix(&fetched, platform);
    if models.is_empty() {
        return Ok(ProviderBranchResult::Skipped);
    }
    let model_ids = models
        .iter()
        .map(|model| model.id.clone())
        .collect::<Vec<_>>();
    let selected_id = pick_default_model(config, provider_id, &model_ids);
    let Some(selected_model) = models.iter().find(|model| model.id == selected_id) else {
        return Ok(ProviderBranchResult::Skipped);
    };
    let mut next = config.clone();
    apply_open_platform_config(
        &mut next,
        platform,
        &models,
        selected_model,
        false,
        None,
        api_key,
    );
    let prefix = format!("{provider_id}/");
    prepare_refreshed_config(config, &mut next, provider_id, &prefix);
    finish_provider_refresh(
        host,
        config,
        next,
        provider_id,
        platform.name,
        &prefix,
        false,
    )
    .await
}

async fn refresh_managed_api_key(
    host: &dyn RefreshProviderHost,
    config: &Map<String, Value>,
    provider_id: &str,
    provider: &Map<String, Value>,
) -> Result<ProviderBranchResult, RefreshOperationError> {
    let Some(api_key) = resolve_provider_api_key(provider) else {
        return Ok(ProviderBranchResult::Skipped);
    };
    let base_url = provider.get("baseUrl").and_then(Value::as_str);
    let models = fetch_managed_kimi_code_models(&api_key, base_url, None, CredentialKind::ApiKey)
        .await
        .map_err(RefreshOperationError::ManagedModels)?;
    if models.is_empty() {
        return Ok(ProviderBranchResult::Skipped);
    }
    let prefix = if provider_id == KIMI_CODE_PROVIDER_NAME {
        format!("{KIMI_CODE_PLATFORM_ID}/")
    } else {
        format!("{provider_id}/")
    };
    let mut next = config.clone();
    apply_managed_api_key_provider_models(&mut next, provider_id, &models, &prefix)
        .map_err(RefreshOperationError::ManagedConfig)?;
    prepare_refreshed_config(config, &mut next, provider_id, &prefix);
    finish_provider_refresh(host, config, next, provider_id, provider_id, &prefix, true).await
}

fn prepare_refreshed_config(
    config: &Map<String, Value>,
    next: &mut Map<String, Value>,
    provider_id: &str,
    alias_prefix: &str,
) {
    let refreshed_alias_keys = provider_refresh_alias_keys(config, next, provider_id, alias_prefix);
    restore_provider_aliases(
        next,
        preserve_user_provider_aliases(config, provider_id, &refreshed_alias_keys),
    );
    let default_model = config.get("defaultModel").and_then(Value::as_str);
    let default_enabled = config
        .get("thinking")
        .and_then(Value::as_object)
        .and_then(|thinking| thinking.get("enabled"))
        .and_then(Value::as_bool);
    restore_default_selection(next, default_model, default_enabled);
    clamp_dangling_default(next);
    clear_default_thinking_when_default_removed(next, default_model);
}

async fn finish_provider_refresh(
    host: &dyn RefreshProviderHost,
    config: &Map<String, Value>,
    next: Map<String, Value>,
    provider_id: &str,
    provider_name: &str,
    alias_prefix: &str,
    include_default_provider: bool,
) -> Result<ProviderBranchResult, RefreshOperationError> {
    let refreshed_alias_keys =
        provider_refresh_alias_keys(config, &next, provider_id, alias_prefix);
    if provider_models_equal(config, &next, provider_id, &refreshed_alias_keys) {
        return Ok(ProviderBranchResult::Unchanged(provider_id.to_owned()));
    }
    let (added, removed) = compute_changes(
        &collect_model_ids_for_aliases(config, &refreshed_alias_keys),
        &collect_model_ids_for_aliases(&next, &refreshed_alias_keys),
    );
    host.remove_provider(provider_id)
        .await
        .map_err(RefreshOperationError::Host)?;
    let patch = config_patch(&next, include_default_provider);
    let persisted = host
        .set_config(patch)
        .await
        .map_err(RefreshOperationError::Host)?;
    Ok(ProviderBranchResult::Changed {
        config: persisted,
        change: ProviderChange {
            provider_id: provider_id.to_owned(),
            provider_name: provider_name.to_owned(),
            added,
            removed,
        },
    })
}

#[derive(Clone)]
struct CustomRegistryGroup {
    sources: Vec<CustomRegistrySource>,
    source_keys: HashSet<String>,
    provider_ids: Vec<String>,
}

struct CustomGroupResult {
    config: Map<String, Value>,
    changed: Vec<ProviderChange>,
    unchanged: Vec<String>,
}

fn collect_custom_registry_groups(
    config: &Map<String, Value>,
) -> IndexMap<String, CustomRegistryGroup> {
    let mut groups = IndexMap::<String, CustomRegistryGroup>::new();
    for provider_id in provider_ids(config) {
        if provider_id == KIMI_CODE_PROVIDER_NAME || is_open_platform_id(&provider_id) {
            continue;
        }
        let Some(provider) = read_provider_object(config, &provider_id) else {
            continue;
        };
        let Some(source) = read_custom_registry_source(provider) else {
            continue;
        };
        let group_key = custom_registry_source_key(&source);
        let credential_key = custom_registry_source_credential_key(&source);
        if let Some(group) = groups.get_mut(&group_key) {
            if group.source_keys.insert(credential_key) {
                group.sources.push(source);
            }
            group.provider_ids.push(provider_id);
        } else {
            groups.insert(
                group_key,
                CustomRegistryGroup {
                    sources: vec![source],
                    source_keys: HashSet::from([credential_key]),
                    provider_ids: vec![provider_id],
                },
            );
        }
    }
    groups
}

async fn fetch_custom_registry_from_sources(
    sources: &[CustomRegistrySource],
    user_agent: Option<&str>,
) -> Result<
    (
        IndexMap<String, CustomRegistryProviderEntry>,
        CustomRegistrySource,
    ),
    RefreshOperationError,
> {
    let mut last_error = None;
    for source in sources {
        match fetch_custom_registry(source, user_agent).await {
            Ok(entries) => return Ok((entries, source.clone())),
            Err(error) => last_error = Some(error),
        }
    }
    Err(RefreshOperationError::CustomRegistry(last_error.unwrap_or(
        CustomRegistryError::UnexpectedResponse(
            "No custom registry sources configured.".to_owned(),
        ),
    )))
}

async fn refresh_custom_registry_group(
    host: &dyn RefreshProviderHost,
    config: &Map<String, Value>,
    group: &CustomRegistryGroup,
    target_id: Option<&str>,
) -> Result<Option<CustomGroupResult>, RefreshOperationError> {
    let (entries, source) =
        fetch_custom_registry_from_sources(&group.sources, host.user_agent()).await?;
    let mut next = config.clone();
    let remote_entries = entries.values().cloned().collect::<Vec<_>>();
    let remote_by_id = remote_entries
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<std::collections::HashMap<_, _>>();
    let mut provider_ids_to_sync = group.provider_ids.iter().cloned().collect::<IndexSet<_>>();
    if target_id.is_none() {
        provider_ids_to_sync.extend(remote_entries.iter().map(|entry| entry.id.clone()));
    }
    let mut changed = Vec::new();
    let mut unchanged = Vec::new();
    let mut providers_to_remove = IndexSet::new();
    let mut has_unreported_config_change = false;
    for provider_id in provider_ids_to_sync {
        if target_id.is_some_and(|target| target != provider_id) {
            continue;
        }
        let Some(entry) = remote_by_id.get(provider_id.as_str()).copied() else {
            let old_ids =
                collect_model_ids_for_aliases(config, &provider_alias_keys(config, &provider_id));
            remove_custom_registry_provider(&mut next, &provider_id);
            changed.push(ProviderChange {
                provider_id: provider_id.clone(),
                provider_name: provider_id.clone(),
                added: 0,
                removed: old_ids.len(),
            });
            providers_to_remove.insert(provider_id);
            continue;
        };
        let existed = read_provider(config, &provider_id).is_some();
        apply_custom_registry_provider(&mut next, entry, &source);
        let prefix = format!("{provider_id}/");
        let refreshed_alias_keys =
            provider_refresh_alias_keys(config, &next, &provider_id, &prefix);
        if existed {
            restore_provider_aliases(
                &mut next,
                preserve_user_provider_aliases(config, &provider_id, &refreshed_alias_keys),
            );
        }
        let models_equal =
            existed && provider_models_equal(config, &next, &provider_id, &refreshed_alias_keys);
        if models_equal && provider_config_equal(config, &next, &provider_id) {
            unchanged.push(provider_id);
        } else if models_equal {
            unchanged.push(provider_id.clone());
            providers_to_remove.insert(provider_id);
            has_unreported_config_change = true;
        } else {
            let (added, removed) = compute_changes(
                &collect_model_ids_for_aliases(config, &refreshed_alias_keys),
                &collect_model_ids_for_aliases(&next, &refreshed_alias_keys),
            );
            changed.push(ProviderChange {
                provider_id: provider_id.clone(),
                provider_name: if entry.name.is_empty() {
                    provider_id.clone()
                } else {
                    entry.name.clone()
                },
                added,
                removed,
            });
            if existed {
                providers_to_remove.insert(provider_id);
            }
        }
    }

    if changed.is_empty() && !has_unreported_config_change {
        return Ok(None);
    }
    let default_model = config.get("defaultModel").and_then(Value::as_str);
    let default_enabled = config
        .get("thinking")
        .and_then(Value::as_object)
        .and_then(|thinking| thinking.get("enabled"))
        .and_then(Value::as_bool);
    restore_default_selection(&mut next, default_model, default_enabled);
    clamp_dangling_default(&mut next);
    clear_default_thinking_when_default_removed(&mut next, default_model);
    for provider_id in providers_to_remove {
        host.remove_provider(&provider_id)
            .await
            .map_err(RefreshOperationError::Host)?;
    }
    let persisted = host
        .set_config(config_patch(&next, false))
        .await
        .map_err(RefreshOperationError::Host)?;
    Ok(Some(CustomGroupResult {
        config: persisted,
        changed,
        unchanged,
    }))
}

fn config_patch(config: &Map<String, Value>, include_default_provider: bool) -> Map<String, Value> {
    let mut patch = Map::new();
    for key in ["providers", "models", "defaultModel", "thinking"] {
        if let Some(value) = config.get(key) {
            patch.insert(key.to_owned(), value.clone());
        }
    }
    if include_default_provider && let Some(value) = config.get("defaultProvider") {
        patch.insert("defaultProvider".to_owned(), value.clone());
    }
    patch
}

fn provider_ids(config: &Map<String, Value>) -> Vec<String> {
    config
        .get("providers")
        .and_then(Value::as_object)
        .map(|providers| providers.keys().cloned().collect())
        .unwrap_or_default()
}

fn read_provider_object<'a>(
    config: &'a Map<String, Value>,
    provider_id: &str,
) -> Option<&'a Map<String, Value>> {
    read_provider(config, provider_id).and_then(Value::as_object)
}

fn parse_oauth_ref_input(value: &Value) -> Option<ManagedKimiOAuthRefInput> {
    let value = value.as_object()?;
    Some(ManagedKimiOAuthRefInput {
        storage: match value.get("storage").and_then(Value::as_str) {
            Some("file") => Some(OAuthStorageBackend::File),
            Some("keyring") => Some(OAuthStorageBackend::Keyring),
            _ => None,
        },
        key: value.get("key").and_then(Value::as_str).map(str::to_owned),
        oauth_host: value
            .get("oauthHost")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

pub(crate) fn resolve_provider_api_key(provider: &Map<String, Value>) -> Option<String> {
    provider
        .get("apiKey")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            provider
                .get("env")
                .and_then(Value::as_object)
                .and_then(|environment| environment.get("KIMI_API_KEY"))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
}

pub(crate) fn read_custom_registry_source(
    provider: &Map<String, Value>,
) -> Option<CustomRegistrySource> {
    let source = provider.get("source")?.as_object()?;
    if source.get("kind")?.as_str()? != "apiJson" {
        return None;
    }
    let url = source.get("url")?.as_str()?;
    if url.is_empty() {
        return None;
    }
    Some(CustomRegistrySource {
        url: url.to_owned(),
        api_key: source.get("apiKey")?.as_str()?.to_owned(),
    })
}

pub(crate) fn custom_registry_source_key(source: &CustomRegistrySource) -> String {
    serde_json::json!([source.url]).to_string()
}

pub(crate) fn custom_registry_source_credential_key(source: &CustomRegistrySource) -> String {
    serde_json::json!([source.url, source.api_key]).to_string()
}

pub(crate) fn collect_model_ids_for_aliases(
    config: &Map<String, Value>,
    alias_keys: &HashSet<String>,
) -> HashSet<String> {
    alias_keys
        .iter()
        .filter_map(|alias_key| read_model(config, alias_key))
        .filter_map(|model| model.get("model").and_then(Value::as_str))
        .filter(|model_id| !model_id.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(crate) fn provider_alias_keys(
    config: &Map<String, Value>,
    provider_id: &str,
) -> HashSet<String> {
    config
        .get("models")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|models| models.iter())
        .filter(|(_, model)| model.get("provider").and_then(Value::as_str) == Some(provider_id))
        .map(|(alias, _)| alias.clone())
        .collect()
}

pub(crate) fn generated_provider_alias_keys(
    config: &Map<String, Value>,
    provider_id: &str,
    alias_prefix: &str,
) -> HashSet<String> {
    provider_alias_keys(config, provider_id)
        .into_iter()
        .filter(|alias| alias.starts_with(alias_prefix))
        .collect()
}

pub(crate) fn compute_changes(
    old_ids: &HashSet<String>,
    new_ids: &HashSet<String>,
) -> (usize, usize) {
    (
        new_ids.difference(old_ids).count(),
        old_ids.difference(new_ids).count(),
    )
}

pub(crate) fn provider_models_equal(
    config: &Map<String, Value>,
    next_config: &Map<String, Value>,
    provider_id: &str,
    alias_keys: &HashSet<String>,
) -> bool {
    provider_model_snapshot(config, provider_id, alias_keys)
        == provider_model_snapshot(next_config, provider_id, alias_keys)
}

pub(crate) fn provider_config_equal(
    config: &Map<String, Value>,
    next_config: &Map<String, Value>,
    provider_id: &str,
) -> bool {
    read_provider(config, provider_id) == read_provider(next_config, provider_id)
}

pub(crate) fn provider_refresh_alias_keys(
    config: &Map<String, Value>,
    next_config: &Map<String, Value>,
    provider_id: &str,
    alias_prefix: &str,
) -> HashSet<String> {
    let mut keys = generated_provider_alias_keys(config, provider_id, alias_prefix);
    keys.extend(provider_alias_keys(next_config, provider_id));
    keys
}

pub(crate) fn preserve_user_provider_aliases(
    config: &Map<String, Value>,
    provider_id: &str,
    refreshed_alias_keys: &HashSet<String>,
) -> Map<String, Value> {
    config
        .get("models")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|models| models.iter())
        .filter(|(alias, model)| {
            model.get("provider").and_then(Value::as_str) == Some(provider_id)
                && !refreshed_alias_keys.contains(*alias)
        })
        .map(|(alias, model)| (alias.clone(), model.clone()))
        .collect()
}

pub(crate) fn restore_provider_aliases(
    config: &mut Map<String, Value>,
    aliases: Map<String, Value>,
) {
    if aliases.is_empty() {
        return;
    }
    let models = config
        .entry("models")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(models) = models.as_object_mut() {
        models.extend(aliases);
    }
}

pub(crate) fn restore_default_selection(
    config: &mut Map<String, Value>,
    default_model: Option<&str>,
    default_enabled: Option<bool>,
) {
    let Some(default_model) = default_model.filter(|model| read_model(config, model).is_some())
    else {
        return;
    };
    config.insert(
        "defaultModel".to_owned(),
        Value::String(default_model.to_owned()),
    );
    let always_thinking = read_model(config, default_model)
        .and_then(|model| model.get("capabilities"))
        .and_then(Value::as_array)
        .is_some_and(|capabilities| {
            capabilities
                .iter()
                .any(|capability| capability.as_str() == Some("always_thinking"))
        });
    let enabled = always_thinking.then_some(true).or(default_enabled);
    if let Some(enabled) = enabled {
        let mut thinking = config
            .remove("thinking")
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        thinking.insert("enabled".to_owned(), Value::Bool(enabled));
        config.insert("thinking".to_owned(), Value::Object(thinking));
    }
}

pub(crate) fn clamp_dangling_default(config: &mut Map<String, Value>) {
    let dangling = config
        .get("defaultModel")
        .and_then(Value::as_str)
        .is_some_and(|default_model| read_model(config, default_model).is_none());
    if dangling {
        config.remove("defaultModel");
        config.remove("thinking");
    }
}

pub(crate) fn clear_default_thinking_when_default_removed(
    config: &mut Map<String, Value>,
    previous_default_model: Option<&str>,
) {
    if previous_default_model.is_some() && !config.contains_key("defaultModel") {
        config.remove("thinking");
    }
}

pub(crate) fn pick_default_model(
    config: &Map<String, Value>,
    provider_id: &str,
    model_ids: &[String],
) -> String {
    let Some(first_model) = model_ids.first() else {
        return String::new();
    };
    let existing_model_id = config
        .get("defaultModel")
        .and_then(Value::as_str)
        .and_then(|default_model| read_model(config, default_model))
        .filter(|alias| alias.get("provider").and_then(Value::as_str) == Some(provider_id))
        .and_then(|alias| alias.get("model"))
        .and_then(Value::as_str);
    existing_model_id
        .filter(|model_id| model_ids.iter().any(|available| available == model_id))
        .unwrap_or(first_model)
        .to_owned()
}

fn read_provider<'a>(config: &'a Map<String, Value>, provider_id: &str) -> Option<&'a Value> {
    config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get(provider_id))
}

fn read_model<'a>(config: &'a Map<String, Value>, alias: &str) -> Option<&'a Map<String, Value>> {
    config
        .get("models")
        .and_then(Value::as_object)
        .and_then(|models| models.get(alias))
        .and_then(Value::as_object)
}

fn provider_model_snapshot(
    config: &Map<String, Value>,
    provider_id: &str,
    alias_keys: &HashSet<String>,
) -> String {
    let mut snapshots = alias_keys
        .iter()
        .filter_map(|alias| {
            let model = read_model(config, alias)?;
            if model.get("provider").and_then(Value::as_str) != Some(provider_id) {
                return None;
            }
            let mut model = model.clone();
            if let Some(capabilities) = model.get_mut("capabilities").and_then(Value::as_array_mut)
            {
                capabilities.sort_by(|left, right| {
                    left.as_str()
                        .unwrap_or_default()
                        .cmp(right.as_str().unwrap_or_default())
                });
            }
            Some((alias.clone(), Value::Object(model)))
        })
        .collect::<Vec<_>>();
    snapshots.sort_by(|left, right| left.0.cmp(&right.0));
    Value::Array(
        snapshots
            .into_iter()
            .map(|(alias, model)| serde_json::json!({ "alias": alias, "model": model }))
            .collect(),
    )
    .to_string()
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::Mutex,
        thread,
    };

    use super::*;
    use crate::oauth::custom_registry::remove_custom_registry_provider;

    #[derive(Debug)]
    struct TestHostError(String);

    impl fmt::Display for TestHostError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(&self.0)
        }
    }

    impl Error for TestHostError {}

    struct TestHost {
        config: Mutex<Map<String, Value>>,
        events: Mutex<Vec<String>>,
        oauth_refs: Mutex<Vec<ManagedKimiOAuthRef>>,
        access_token: String,
    }

    #[async_trait]
    impl RefreshProviderHost for TestHost {
        async fn get_config(&self) -> Result<Map<String, Value>, RefreshHostError> {
            Ok(self
                .config
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone())
        }

        async fn remove_provider(
            &self,
            provider_id: &str,
        ) -> Result<Map<String, Value>, RefreshHostError> {
            self.events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("remove:{provider_id}"));
            let mut config = self
                .config
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            remove_custom_registry_provider(&mut config, provider_id);
            Ok(config.clone())
        }

        async fn set_config(
            &self,
            patch: Map<String, Value>,
        ) -> Result<Map<String, Value>, RefreshHostError> {
            self.events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push("set".to_owned());
            let mut config = self
                .config
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            config.extend(patch);
            Ok(config.clone())
        }

        async fn resolve_oauth_token(
            &self,
            _provider_name: &str,
            oauth_ref: Option<&ManagedKimiOAuthRef>,
        ) -> Result<String, RefreshHostError> {
            let reference = oauth_ref.ok_or_else(|| {
                RefreshHostError::new(TestHostError("missing OAuth ref".to_owned()))
            })?;
            self.oauth_refs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(reference.clone());
            Ok(self.access_token.clone())
        }
    }

    fn sequence_server(responses: Vec<(u16, &'static str)>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind refresh server");
        let address = listener.local_addr().expect("refresh server address");
        let handle = thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().expect("accept refresh request");
                let mut bytes = Vec::new();
                let mut buffer = [0_u8; 4_096];
                loop {
                    let count = stream.read(&mut buffer).expect("read refresh request");
                    if count == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..count]);
                    if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let response = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write refresh response");
            }
        });
        (format!("http://{address}/coding/v1"), handle)
    }

    #[test]
    fn resolves_inline_and_environment_api_keys_in_runtime_order() {
        let inline = serde_json::json!({
            "apiKey": "inline",
            "env": { "KIMI_API_KEY": "environment" }
        });
        assert_eq!(
            resolve_provider_api_key(inline.as_object().expect("provider")),
            Some("inline".to_owned())
        );
        let environment = serde_json::json!({
            "apiKey": "",
            "env": { "KIMI_API_KEY": "environment" }
        });
        assert_eq!(
            resolve_provider_api_key(environment.as_object().expect("provider")),
            Some("environment".to_owned())
        );
    }

    #[test]
    fn reads_only_complete_api_json_sources_and_builds_stable_keys() {
        let provider = serde_json::json!({
            "source": {
                "kind": "apiJson",
                "url": "https://registry.example/api.json",
                "apiKey": "secret"
            }
        });
        let source =
            read_custom_registry_source(provider.as_object().expect("provider")).expect("source");
        assert_eq!(
            custom_registry_source_key(&source),
            r#"["https://registry.example/api.json"]"#
        );
        assert_eq!(
            custom_registry_source_credential_key(&source),
            r#"["https://registry.example/api.json","secret"]"#
        );
        let incomplete = serde_json::json!({
            "source": { "kind": "apiJson", "url": "https://registry.example/api.json" }
        });
        assert_eq!(
            read_custom_registry_source(incomplete.as_object().expect("provider")),
            None
        );
    }

    #[test]
    fn model_snapshots_compare_metadata_but_ignore_capability_order() {
        let first = serde_json::json!({
            "providers": {},
            "models": {
                "p/a": {
                    "provider": "p", "model": "a", "maxContextSize": 100,
                    "capabilities": ["thinking", "tool_use"]
                }
            }
        });
        let reordered = serde_json::json!({
            "providers": {},
            "models": {
                "p/a": {
                    "provider": "p", "model": "a", "maxContextSize": 100,
                    "capabilities": ["tool_use", "thinking"]
                }
            }
        });
        let changed = serde_json::json!({
            "providers": {},
            "models": {
                "p/a": {
                    "provider": "p", "model": "a", "maxContextSize": 200,
                    "capabilities": ["tool_use", "thinking"]
                }
            }
        });
        let aliases = HashSet::from(["p/a".to_owned()]);
        assert!(provider_models_equal(
            first.as_object().expect("config"),
            reordered.as_object().expect("config"),
            "p",
            &aliases
        ));
        assert!(!provider_models_equal(
            first.as_object().expect("config"),
            changed.as_object().expect("config"),
            "p",
            &aliases
        ));
    }

    #[test]
    fn refresh_keys_preserve_user_aliases_outside_generated_prefix() {
        let config = serde_json::json!({
            "models": {
                "p/old": { "provider": "p", "model": "old" },
                "custom-alias": { "provider": "p", "model": "old" }
            }
        });
        let next = serde_json::json!({
            "models": { "p/new": { "provider": "p", "model": "new" } }
        });
        let keys = provider_refresh_alias_keys(
            config.as_object().expect("config"),
            next.as_object().expect("config"),
            "p",
            "p/",
        );
        assert_eq!(
            keys,
            HashSet::from(["p/old".to_owned(), "p/new".to_owned()])
        );
        let preserved =
            preserve_user_provider_aliases(config.as_object().expect("config"), "p", &keys);
        assert_eq!(preserved.len(), 1);
        assert!(preserved.contains_key("custom-alias"));
        let old_ids = collect_model_ids_for_aliases(config.as_object().expect("config"), &keys);
        let new_ids = collect_model_ids_for_aliases(next.as_object().expect("config"), &keys);
        assert_eq!(compute_changes(&old_ids, &new_ids), (1, 1));
    }

    #[test]
    fn restores_valid_default_and_never_disables_always_thinking() {
        let mut config = serde_json::json!({
            "models": {
                "p/model": {
                    "provider": "p", "model": "model",
                    "capabilities": ["always_thinking", "thinking"]
                }
            },
            "thinking": { "enabled": false, "effort": "high" }
        });
        restore_default_selection(
            config.as_object_mut().expect("config"),
            Some("p/model"),
            Some(false),
        );
        assert_eq!(config["defaultModel"], "p/model");
        assert_eq!(config["thinking"]["enabled"], true);
        assert_eq!(config["thinking"]["effort"], "high");

        config["defaultModel"] = Value::String("missing".to_owned());
        clamp_dangling_default(config.as_object_mut().expect("config"));
        assert!(config.get("defaultModel").is_none());
        assert!(config.get("thinking").is_none());
    }

    #[test]
    fn picks_existing_provider_default_when_model_remains_available() {
        let config = serde_json::json!({
            "models": {
                "alias": { "provider": "p", "model": "second" }
            },
            "defaultModel": "alias"
        });
        let models = vec!["first".to_owned(), "second".to_owned()];
        assert_eq!(
            pick_default_model(config.as_object().expect("config"), "p", &models),
            "second"
        );
        assert_eq!(
            pick_default_model(config.as_object().expect("config"), "other", &models),
            "first"
        );
        assert_eq!(
            pick_default_model(config.as_object().expect("config"), "p", &[]),
            ""
        );
    }

    #[test]
    fn provider_config_comparison_tracks_full_provider_value() {
        let config = serde_json::json!({ "providers": { "p": { "type": "openai", "x": 1 } } });
        let same = config.clone();
        let changed = serde_json::json!({ "providers": { "p": { "type": "openai", "x": 2 } } });
        assert!(provider_config_equal(
            config.as_object().expect("config"),
            same.as_object().expect("config"),
            "p"
        ));
        assert!(!provider_config_equal(
            config.as_object().expect("config"),
            changed.as_object().expect("config"),
            "p"
        ));
    }

    #[test]
    fn restore_aliases_and_clear_thinking_follow_default_removal() {
        let mut config = serde_json::json!({
            "models": {},
            "thinking": { "enabled": true }
        });
        restore_provider_aliases(
            config.as_object_mut().expect("config"),
            Map::from_iter([(
                "custom".to_owned(),
                serde_json::json!({ "provider": "p", "model": "m" }),
            )]),
        );
        assert!(config["models"].get("custom").is_some());
        clear_default_thinking_when_default_removed(
            config.as_object_mut().expect("config"),
            Some("old"),
        );
        assert!(config.get("thinking").is_none());
    }

    #[tokio::test]
    async fn managed_oauth_refresh_writes_changes_then_reports_unchanged() {
        let response = r#"{"data":[{"id":"kimi-k2","context_length":262144,"supports_reasoning":true},{"id":"kimi-new","context_length":131072}]}"#;
        let (base_url, server) = sequence_server(vec![(200, response), (200, response)]);
        let initial = serde_json::json!({
            "providers": {
                "managed:kimi-code": {
                    "type": "kimi",
                    "baseUrl": base_url,
                    "apiKey": "",
                    "oauth": { "storage": "file", "key": "oauth/old" }
                }
            },
            "models": {
                "kimi-code/kimi-k2": {
                    "provider": "managed:kimi-code",
                    "model": "kimi-k2",
                    "maxContextSize": 1000
                },
                "kimi-code/stale": {
                    "provider": "managed:kimi-code",
                    "model": "stale",
                    "maxContextSize": 1000
                },
                "my-kimi": {
                    "provider": "managed:kimi-code",
                    "model": "kimi-k2",
                    "userAlias": true
                }
            },
            "defaultModel": "kimi-code/kimi-k2",
            "thinking": { "enabled": false, "effort": "high" }
        });
        let host = TestHost {
            config: Mutex::new(initial.as_object().expect("config").clone()),
            events: Mutex::new(Vec::new()),
            oauth_refs: Mutex::new(Vec::new()),
            access_token: "access-token".to_owned(),
        };

        let first = refresh_provider_models(
            &host,
            &RefreshProviderOptions {
                scope: RefreshProviderScope::OAuth,
                provider_id: None,
            },
        )
        .await
        .expect("first refresh");
        assert_eq!(
            first.changed,
            vec![ProviderChange {
                provider_id: "managed:kimi-code".to_owned(),
                provider_name: "Kimi Code".to_owned(),
                added: 1,
                removed: 1,
            }]
        );
        assert!(first.failed.is_empty());
        let stored = host
            .config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(
            stored["models"]["kimi-code/kimi-k2"]["maxContextSize"],
            262_144
        );
        assert!(stored["models"].get("kimi-code/kimi-new").is_some());
        assert!(stored["models"].get("kimi-code/stale").is_none());
        assert_eq!(stored["models"]["my-kimi"]["userAlias"], true);
        assert_eq!(stored["defaultModel"], "kimi-code/kimi-k2");
        assert_eq!(stored["thinking"]["enabled"], false);
        assert_eq!(stored["thinking"]["effort"], "high");

        let second = refresh_provider_models(
            &host,
            &RefreshProviderOptions {
                scope: RefreshProviderScope::OAuth,
                provider_id: None,
            },
        )
        .await
        .expect("second refresh");
        server.join().expect("refresh server thread");
        assert!(second.changed.is_empty());
        assert_eq!(second.unchanged, ["managed:kimi-code"]);
        assert_eq!(
            host.events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            ["remove:managed:kimi-code", "set"]
        );
        assert_eq!(
            host.oauth_refs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn provider_failure_is_collected_without_rejecting_refresh() {
        let (base_url, server) = sequence_server(vec![(500, r#"{"error":{"message":"down"}}"#)]);
        let initial = serde_json::json!({
            "providers": {
                "managed:kimi-code": {
                    "type": "kimi",
                    "baseUrl": base_url,
                    "oauth": { "storage": "file", "key": "oauth/old" }
                }
            }
        });
        let host = TestHost {
            config: Mutex::new(initial.as_object().expect("config").clone()),
            events: Mutex::new(Vec::new()),
            oauth_refs: Mutex::new(Vec::new()),
            access_token: "access-token".to_owned(),
        };

        let result = refresh_provider_models(
            &host,
            &RefreshProviderOptions {
                scope: RefreshProviderScope::OAuth,
                provider_id: None,
            },
        )
        .await
        .expect("refresh result");
        server.join().expect("refresh server thread");

        assert!(result.changed.is_empty());
        assert!(result.unchanged.is_empty());
        assert_eq!(result.failed.len(), 1);
        assert_eq!(result.failed[0].provider, "managed:kimi-code");
        assert_eq!(result.failed[0].reason, "down");
        assert!(
            host.events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
    }

    #[tokio::test]
    async fn oauth_scope_skips_non_oauth_registry_groups() {
        let initial = serde_json::json!({
            "providers": {
                "registry": {
                    "type": "openai",
                    "source": {
                        "kind": "apiJson",
                        "url": "http://127.0.0.1:1/api.json",
                        "apiKey": ""
                    }
                }
            }
        });
        let host = TestHost {
            config: Mutex::new(initial.as_object().expect("config").clone()),
            events: Mutex::new(Vec::new()),
            oauth_refs: Mutex::new(Vec::new()),
            access_token: String::new(),
        };

        let result = refresh_provider_models(
            &host,
            &RefreshProviderOptions {
                scope: RefreshProviderScope::OAuth,
                provider_id: None,
            },
        )
        .await
        .expect("oauth-only refresh");
        assert_eq!(result, RefreshResult::default());
    }
}
