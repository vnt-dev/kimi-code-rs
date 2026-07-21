use std::collections::BTreeMap;

use async_trait::async_trait;
use indexmap::IndexMap;
use serde_json::Value;

use crate::{
    oauth::managed_auth::KIMI_CODE_PROVIDER_NAME,
    sdk::{model_alias::ModelAlias, types::ThinkingEffort},
    tui::utils::thinking_config::{ThinkingConfigPatch, thinking_effort_to_config},
};

use super::config::{ModelCommandState, effective_model_for_state};

pub const DEFAULT_OAUTH_PROVIDER_NAME: &str = KIMI_CODE_PROVIDER_NAME;

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderCommandState {
    pub current_model: String,
    pub current_thinking_effort: ThinkingEffort,
    pub available_providers: IndexMap<String, Value>,
    pub available_models: IndexMap<String, ModelAlias>,
    pub model_state: ModelCommandState,
}

impl ProviderCommandState {
    pub fn active_provider_id(&self) -> Option<&str> {
        self.available_models
            .get(&self.current_model)
            .map(|model| model.provider.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderManagerAction {
    Add,
    DeleteSource(Vec<String>),
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAddSource {
    KnownCatalog,
    CustomRegistry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAddResult {
    pub provider_ids: Vec<String>,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderModelSelection {
    pub alias: String,
    pub thinking: ThinkingEffort,
}

#[async_trait(?Send)]
pub trait ProviderCommandHost {
    fn provider_command_state(&self) -> ProviderCommandState;
    async fn pick_provider_manager_action(
        &mut self,
        providers: IndexMap<String, Value>,
        active_provider_id: Option<String>,
    ) -> ProviderManagerAction;
    async fn pick_provider_add_source(&mut self) -> Option<ProviderAddSource>;

    /// Uses the migrated catalog fetch/apply helpers, persists the provider and
    /// refreshes runtime config before returning.
    async fn add_known_catalog_provider(&mut self) -> Result<Option<ProviderAddResult>, String>;
    /// Uses the migrated custom-registry fetch/apply helpers and persists all
    /// entries from the selected registry.
    async fn add_custom_registry(&mut self) -> Result<Option<ProviderAddResult>, String>;
    async fn pick_provider_default_model(
        &mut self,
        provider_ids: &[String],
        models: IndexMap<String, ModelAlias>,
        current_model: &str,
        current_thinking_effort: &ThinkingEffort,
    ) -> Option<ProviderModelSelection>;

    async fn oauth_logout(&mut self, provider_name: &str) -> Result<(), String>;
    async fn remove_provider(&mut self, provider_id: &str) -> Result<(), String>;
    async fn refresh_config_after_login(&mut self) -> Result<(), String>;
    async fn refresh_config_after_logout(&mut self) -> Result<(), String>;
    async fn clear_active_session_after_logout(&mut self) -> Result<(), String>;
    async fn reload_available_provider_state(&mut self) -> Result<(), String>;
    async fn persist_default_model(
        &mut self,
        alias: &str,
        thinking: ThinkingConfigPatch,
    ) -> Result<(), String>;

    fn track_provider(&mut self, event: &str, properties: BTreeMap<String, String>);
    fn show_provider_status(&mut self, message: &str);
    fn show_provider_error(&mut self, message: &str);
}

// Original: `src/tui/commands/provider.ts`, `handleProviderCommand()`.
pub async fn handle_provider_command(host: &mut impl ProviderCommandHost) {
    loop {
        let state = host.provider_command_state();
        let active_provider_id = state.active_provider_id().map(str::to_owned);
        let action = host
            .pick_provider_manager_action(state.available_providers, active_provider_id)
            .await;
        match action {
            ProviderManagerAction::Close => return,
            ProviderManagerAction::DeleteSource(provider_ids) => {
                handle_provider_manager_delete_source(host, &provider_ids).await;
            }
            ProviderManagerAction::Add => {
                let Some(source) = host.pick_provider_add_source().await else {
                    continue;
                };
                let result = match source {
                    ProviderAddSource::KnownCatalog => host.add_known_catalog_provider().await,
                    ProviderAddSource::CustomRegistry => host.add_custom_registry().await,
                };
                match result {
                    Ok(Some(added)) => finish_provider_add(host, source, added).await,
                    Ok(None) => {}
                    Err(error) => {
                        host.show_provider_error(&format!("Add provider failed: {error}"))
                    }
                }
            }
        }
    }
}

// Original: `handleProviderManagerDeleteSource()`.
async fn handle_provider_manager_delete_source(
    host: &mut impl ProviderCommandHost,
    provider_ids: &[String],
) {
    for provider_id in provider_ids {
        if let Err(error) = handle_provider_delete(host, provider_id).await {
            host.show_provider_error(&format!("Failed to delete provider {provider_id}: {error}"));
        }
    }
}

// Original: `handleProviderDelete()`.
pub async fn handle_provider_delete(
    host: &mut impl ProviderCommandHost,
    provider_id: &str,
) -> Result<(), String> {
    let active_provider = host
        .provider_command_state()
        .active_provider_id()
        .map(str::to_owned);
    if provider_id == DEFAULT_OAUTH_PROVIDER_NAME {
        host.oauth_logout(DEFAULT_OAUTH_PROVIDER_NAME).await?;
        host.refresh_config_after_logout().await?;
        host.clear_active_session_after_logout().await?;
        return Ok(());
    }

    host.remove_provider(provider_id).await?;
    if active_provider.as_deref() == Some(provider_id) {
        host.refresh_config_after_logout().await?;
        host.clear_active_session_after_logout().await?;
    } else {
        host.reload_available_provider_state().await?;
    }
    Ok(())
}

async fn finish_provider_add(
    host: &mut impl ProviderCommandHost,
    source: ProviderAddSource,
    added: ProviderAddResult,
) {
    if added.provider_ids.is_empty() {
        if source == ProviderAddSource::CustomRegistry {
            host.show_provider_status("Registry contained no providers.");
        }
        return;
    }
    let label = added.display_name.unwrap_or_else(|| {
        if added.provider_ids.len() == 1 {
            added.provider_ids[0].clone()
        } else {
            format!("{} providers", added.provider_ids.len())
        }
    });
    match source {
        ProviderAddSource::KnownCatalog => {
            host.track_provider(
                "connect",
                BTreeMap::from([
                    ("provider".to_owned(), added.provider_ids[0].clone()),
                    ("method".to_owned(), "catalog".to_owned()),
                ]),
            );
            host.show_provider_status(&format!("Provider added: {label}"));
        }
        ProviderAddSource::CustomRegistry => host.show_provider_status(&format!(
            "Imported {} from registry.",
            if added.provider_ids.len() == 1 {
                "1 provider".to_owned()
            } else {
                format!("{} providers", added.provider_ids.len())
            }
        )),
    }

    let state = host.provider_command_state();
    if let Some(selection) = host
        .pick_provider_default_model(
            &added.provider_ids,
            state.available_models,
            &state.current_model,
            &state.current_thinking_effort,
        )
        .await
        && let Err(error) = set_default_model(host, &selection).await
    {
        host.show_provider_error(&format!("Set default model failed: {error}"));
    }
}

// Original: `setDefaultModel()`.
async fn set_default_model(
    host: &mut impl ProviderCommandHost,
    selection: &ProviderModelSelection,
) -> Result<(), String> {
    let state = host.provider_command_state();
    let supported_efforts = state
        .available_models
        .get(&selection.alias)
        .map(|model| effective_model_for_state(&state.model_state, model))
        .and_then(|model| model.support_efforts);
    let thinking = thinking_effort_to_config(&selection.thinking, supported_efforts.as_deref());
    host.persist_default_model(&selection.alias, thinking)
        .await?;
    host.refresh_config_after_login().await?;
    host.track_provider(
        "model_switch",
        BTreeMap::from([("model".to_owned(), selection.alias.clone())]),
    );
    host.show_provider_status(&format!(
        "Default model set to {} with thinking {}.",
        selection.alias,
        selection.thinking.as_str()
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use crate::sdk::model_alias::{ModelProtocol, ProviderType};

    use super::*;

    struct Host {
        state: ProviderCommandState,
        actions: VecDeque<ProviderManagerAction>,
        add_sources: VecDeque<Option<ProviderAddSource>>,
        add_result: Option<ProviderAddResult>,
        selection: Option<ProviderModelSelection>,
        operations: Vec<String>,
        statuses: Vec<String>,
        errors: Vec<String>,
    }

    fn alias(provider: &str) -> ModelAlias {
        ModelAlias {
            provider: provider.to_owned(),
            model: "model".to_owned(),
            max_context_size: 128_000,
            max_output_size: None,
            capabilities: Some(vec!["thinking".to_owned()]),
            display_name: None,
            reasoning_key: None,
            protocol: Some(ModelProtocol::Anthropic),
            adaptive_thinking: None,
            support_efforts: Some(vec!["low".to_owned(), "high".to_owned()]),
            default_effort: Some("high".to_owned()),
            beta_api: None,
            overrides: None,
        }
    }

    impl Default for Host {
        fn default() -> Self {
            let available_models = IndexMap::from([("active/model".to_owned(), alias("active"))]);
            Self {
                state: ProviderCommandState {
                    current_model: "active/model".to_owned(),
                    current_thinking_effort: ThinkingEffort::from("on"),
                    available_providers: IndexMap::from([("active".to_owned(), Value::Null)]),
                    available_models: available_models.clone(),
                    model_state: ModelCommandState {
                        model: "active/model".to_owned(),
                        thinking_effort: ThinkingEffort::from("on"),
                        available_models,
                        provider_types: BTreeMap::from([(
                            "active".to_owned(),
                            ProviderType::Anthropic,
                        )]),
                        streaming: false,
                        has_session: true,
                        has_conversation_history: false,
                    },
                },
                actions: VecDeque::from([ProviderManagerAction::Close]),
                add_sources: VecDeque::new(),
                add_result: None,
                selection: None,
                operations: Vec::new(),
                statuses: Vec::new(),
                errors: Vec::new(),
            }
        }
    }

    #[async_trait(?Send)]
    impl ProviderCommandHost for Host {
        fn provider_command_state(&self) -> ProviderCommandState {
            self.state.clone()
        }
        async fn pick_provider_manager_action(
            &mut self,
            _: IndexMap<String, Value>,
            _: Option<String>,
        ) -> ProviderManagerAction {
            self.actions
                .pop_front()
                .unwrap_or(ProviderManagerAction::Close)
        }
        async fn pick_provider_add_source(&mut self) -> Option<ProviderAddSource> {
            self.add_sources.pop_front().flatten()
        }
        async fn add_known_catalog_provider(
            &mut self,
        ) -> Result<Option<ProviderAddResult>, String> {
            Ok(self.add_result.take())
        }
        async fn add_custom_registry(&mut self) -> Result<Option<ProviderAddResult>, String> {
            Ok(self.add_result.take())
        }
        async fn pick_provider_default_model(
            &mut self,
            _: &[String],
            _: IndexMap<String, ModelAlias>,
            _: &str,
            _: &ThinkingEffort,
        ) -> Option<ProviderModelSelection> {
            self.selection.take()
        }
        async fn oauth_logout(&mut self, provider_name: &str) -> Result<(), String> {
            self.operations
                .push(format!("oauth_logout:{provider_name}"));
            Ok(())
        }
        async fn remove_provider(&mut self, provider_id: &str) -> Result<(), String> {
            self.operations.push(format!("remove:{provider_id}"));
            Ok(())
        }
        async fn refresh_config_after_login(&mut self) -> Result<(), String> {
            self.operations.push("refresh_login".to_owned());
            Ok(())
        }
        async fn refresh_config_after_logout(&mut self) -> Result<(), String> {
            self.operations.push("refresh_logout".to_owned());
            Ok(())
        }
        async fn clear_active_session_after_logout(&mut self) -> Result<(), String> {
            self.operations.push("clear_session".to_owned());
            Ok(())
        }
        async fn reload_available_provider_state(&mut self) -> Result<(), String> {
            self.operations.push("reload_state".to_owned());
            Ok(())
        }
        async fn persist_default_model(
            &mut self,
            alias: &str,
            thinking: ThinkingConfigPatch,
        ) -> Result<(), String> {
            self.operations.push(format!(
                "persist:{alias}:{}:{:?}",
                thinking.enabled, thinking.effort
            ));
            Ok(())
        }
        fn track_provider(&mut self, event: &str, _: BTreeMap<String, String>) {
            self.operations.push(format!("track:{event}"));
        }
        fn show_provider_status(&mut self, message: &str) {
            self.statuses.push(message.to_owned());
        }
        fn show_provider_error(&mut self, message: &str) {
            self.errors.push(message.to_owned());
        }
    }

    #[tokio::test]
    async fn deleting_active_provider_refreshes_and_clears_session() {
        let mut host = Host::default();
        handle_provider_delete(&mut host, "active")
            .await
            .expect("delete");
        assert_eq!(
            host.operations,
            ["remove:active", "refresh_logout", "clear_session"]
        );
    }

    #[tokio::test]
    async fn deleting_inactive_provider_only_reloads_available_state() {
        let mut host = Host::default();
        handle_provider_delete(&mut host, "other")
            .await
            .expect("delete");
        assert_eq!(host.operations, ["remove:other", "reload_state"]);
    }

    #[tokio::test]
    async fn managed_provider_logout_always_runs_full_cleanup() {
        let mut host = Host::default();
        handle_provider_delete(&mut host, DEFAULT_OAUTH_PROVIDER_NAME)
            .await
            .expect("logout");
        assert_eq!(
            host.operations,
            [
                "oauth_logout:managed:kimi-code",
                "refresh_logout",
                "clear_session"
            ]
        );
    }

    #[tokio::test]
    async fn add_flow_tracks_provider_and_persists_selected_default() {
        let mut host = Host {
            actions: VecDeque::from([ProviderManagerAction::Add, ProviderManagerAction::Close]),
            add_sources: VecDeque::from([Some(ProviderAddSource::KnownCatalog)]),
            add_result: Some(ProviderAddResult {
                provider_ids: vec!["active".to_owned()],
                display_name: Some("Active".to_owned()),
            }),
            selection: Some(ProviderModelSelection {
                alias: "active/model".to_owned(),
                thinking: ThinkingEffort::from("high"),
            }),
            ..Host::default()
        };
        handle_provider_command(&mut host).await;
        assert_eq!(
            host.operations,
            [
                "track:connect",
                "persist:active/model:true:None",
                "refresh_login",
                "track:model_switch"
            ]
        );
        assert_eq!(
            host.statuses,
            [
                "Provider added: Active",
                "Default model set to active/model with thinking high."
            ]
        );
    }
}
