use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use tokio::sync::oneshot;

use crate::{
    cli::sub::provider::{Catalog, CatalogModel, infer_wire_type},
    oauth::{
        managed_models::{ManagedKimiCodeModelInfo, ManagedKimiCodeProtocol, model_capabilities},
        open_platform::OpenPlatformDefinition,
    },
    sdk::{
        model_alias::{ModelAlias, ModelProtocol},
        types::ThinkingEffort,
    },
    tui::components::{
        Component,
        dialogs::{
            ApiKeyInputDialogComponent, ApiKeyInputResult, ChoiceOption, ChoicePickerComponent,
            ChoicePickerOptions, FeedbackInputDialogComponent, FeedbackInputDialogResult,
            ModelSelection, ModelSelectorComponent, ModelSelectorOptions,
            PlatformSelectorComponent,
        },
    },
};

/// Minimal editor-replacement surface required by interactive command prompts.
/// The concrete TUI owns rendering and input dispatch while a prompt awaits its
/// one-shot result.
pub trait PromptHost {
    fn mount_editor_replacement(&mut self, component: Box<dyn Component>);
    fn restore_editor(&mut self);
    fn show_error(&mut self, message: &str);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackPromptResult {
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackAttachmentLevel {
    None,
    Logs,
    LogsAndCodebase,
}

impl FeedbackAttachmentLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Logs => "logs",
            Self::LogsAndCodebase => "logs+codebase",
        }
    }

    fn from_value(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "logs" => Some(Self::Logs),
            "logs+codebase" => Some(Self::LogsAndCodebase),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPlatformModelSelection {
    pub model: ManagedKimiCodeModelInfo,
    pub thinking: ThinkingEffort,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CatalogModelSelection {
    pub model: CatalogModel,
    pub thinking: ThinkingEffort,
}

type SharedSender<T> = Arc<Mutex<Option<oneshot::Sender<Option<T>>>>>;

fn send_result<T>(sender: &SharedSender<T>, result: Option<T>) {
    if let Some(sender) = sender.lock().expect("prompt sender lock poisoned").take() {
        let _ = sender.send(result);
    }
}

async fn await_prompt<T>(
    host: &mut impl PromptHost,
    component: Box<dyn Component>,
    receiver: oneshot::Receiver<Option<T>>,
) -> Option<T> {
    host.mount_editor_replacement(component);
    let result = receiver.await.unwrap_or(None);
    host.restore_editor();
    result
}

// Original: `src/tui/commands/prompts.ts`, `promptPlatformSelection()`.
pub async fn prompt_platform_selection(host: &mut impl PromptHost) -> Option<String> {
    let (sender, receiver) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(sender)));
    let select_sender = Arc::clone(&sender);
    let cancel_sender = Arc::clone(&sender);
    let component = PlatformSelectorComponent::new(
        move |value| send_result(&select_sender, Some(value)),
        move || send_result(&cancel_sender, None),
    );
    await_prompt(host, Box::new(component), receiver).await
}

// Original: `promptLogoutProviderSelection()`.
pub async fn prompt_logout_provider_selection(
    host: &mut impl PromptHost,
    options: Vec<ChoiceOption>,
    current_value: Option<String>,
) -> Option<String> {
    let (sender, receiver) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(sender)));
    let select_sender = Arc::clone(&sender);
    let cancel_sender = Arc::clone(&sender);
    let mut picker_options = ChoicePickerOptions::new(
        "Select a provider to log out",
        options,
        move |value| send_result(&select_sender, Some(value)),
        move || send_result(&cancel_sender, None),
    );
    picker_options.current_value = current_value;
    await_prompt(
        host,
        Box::new(ChoicePickerComponent::new(picker_options)),
        receiver,
    )
    .await
}

// Original: `promptFeedbackInput()`.
pub async fn prompt_feedback_input(host: &mut impl PromptHost) -> Option<FeedbackPromptResult> {
    let (sender, receiver) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(sender)));
    let component = FeedbackInputDialogComponent::new(move |result| match result {
        FeedbackInputDialogResult::Ok { value } => {
            send_result(&sender, Some(FeedbackPromptResult { value }));
        }
        FeedbackInputDialogResult::Cancel => send_result(&sender, None),
    });
    await_prompt(host, Box::new(component), receiver).await
}

fn feedback_attachment_options() -> Vec<ChoiceOption> {
    vec![
        ChoiceOption::new("none", "No attachment").with_description("Text feedback only"),
        ChoiceOption::new("logs", "Logs only")
            .with_description("Upload wire events and diagnostic logs from this session"),
        ChoiceOption::new("logs+codebase", "Logs + codebase").with_description(
            "Include your codebase for deeper diagnosis. Sensitive files are automatically excluded \
             — e.g. .env, config files, secret keys. We use attachments only for diagnosis and never \
             share them.",
        ),
    ]
}

// Original: `promptFeedbackAttachment()`.
pub async fn prompt_feedback_attachment(
    host: &mut impl PromptHost,
) -> Option<FeedbackAttachmentLevel> {
    let (sender, receiver) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(sender)));
    let select_sender = Arc::clone(&sender);
    let cancel_sender = Arc::clone(&sender);
    let options = ChoicePickerOptions::new(
        "Share diagnostic info to help us investigate?",
        feedback_attachment_options(),
        move |value| send_result(&select_sender, FeedbackAttachmentLevel::from_value(&value)),
        move || send_result(&cancel_sender, None),
    );
    await_prompt(
        host,
        Box::new(ChoicePickerComponent::new(options)),
        receiver,
    )
    .await
}

// Original: `promptApiKey()`.
pub async fn prompt_api_key(
    host: &mut impl PromptHost,
    platform_name: &str,
    subtitle_lines: Option<Vec<String>>,
) -> Option<String> {
    let (sender, receiver) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(sender)));
    let component = ApiKeyInputDialogComponent::new(
        platform_name,
        subtitle_lines.unwrap_or_else(|| {
            vec!["Your key will be saved to ~/.kimi-code/config.toml".to_owned()]
        }),
        move |result| match result {
            ApiKeyInputResult::Ok { value } => send_result(&sender, Some(value)),
            ApiKeyInputResult::Cancel => send_result(&sender, None),
        },
    );
    await_prompt(host, Box::new(component), receiver).await
}

// Original: `promptCatalogProviderSelection()`.
pub async fn prompt_catalog_provider_selection(
    host: &mut impl PromptHost,
    catalog: &Catalog,
) -> Option<String> {
    let mut options = catalog
        .iter()
        .filter(|(_, entry)| infer_wire_type(entry).is_some())
        .map(|(id, entry)| {
            let mut option = ChoiceOption::new(id, entry.name.as_deref().unwrap_or(id));
            if let Some(api) = entry.api.as_deref().filter(|api| !api.is_empty()) {
                option = option.with_description(api);
            }
            option
        })
        .collect::<Vec<_>>();
    options.sort_by(|left, right| left.label.cmp(&right.label));
    if options.is_empty() {
        host.show_error("Catalog has no providers with supported wire types.");
        return None;
    }

    let (sender, receiver) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(sender)));
    let select_sender = Arc::clone(&sender);
    let cancel_sender = Arc::clone(&sender);
    let mut picker_options = ChoicePickerOptions::new(
        "Select a provider",
        options,
        move |value| send_result(&select_sender, Some(value)),
        move || send_result(&cancel_sender, None),
    );
    picker_options.searchable = true;
    await_prompt(
        host,
        Box::new(ChoicePickerComponent::new(picker_options)),
        receiver,
    )
    .await
}

// Original: `promptModelSelectionForOpenPlatform()`.
pub async fn prompt_model_selection_for_open_platform(
    host: &mut impl PromptHost,
    models: &[ManagedKimiCodeModelInfo],
    platform: &OpenPlatformDefinition,
) -> Option<OpenPlatformModelSelection> {
    let aliases = models
        .iter()
        .map(|model| {
            (
                format!("{}/{}", platform.id, model.id),
                managed_model_alias(platform.id, model),
            )
        })
        .collect();
    let selection = run_model_selector(host, aliases).await?;
    models
        .iter()
        .find(|model| format!("{}/{}", platform.id, model.id) == selection.alias)
        .cloned()
        .map(|model| OpenPlatformModelSelection {
            model,
            thinking: selection.thinking,
        })
}

// Original: `promptModelSelectionForCatalog()`.
pub async fn prompt_model_selection_for_catalog(
    host: &mut impl PromptHost,
    provider_id: &str,
    models: &[CatalogModel],
) -> Option<CatalogModelSelection> {
    let aliases = models
        .iter()
        .map(|model| {
            (
                format!("{provider_id}/{}", model.id),
                catalog_model_alias(provider_id, model),
            )
        })
        .collect();
    let selection = run_model_selector(host, aliases).await?;
    models
        .iter()
        .find(|model| format!("{provider_id}/{}", model.id) == selection.alias)
        .cloned()
        .map(|model| CatalogModelSelection {
            model,
            thinking: selection.thinking,
        })
}

// Original: `runModelSelector()`.
pub async fn run_model_selector(
    host: &mut impl PromptHost,
    models: IndexMap<String, ModelAlias>,
) -> Option<ModelSelection> {
    let first_alias = models
        .first()
        .map(|(alias, _)| alias.clone())
        .unwrap_or_default();
    let initial_thinking = models
        .get(&first_alias)
        .and_then(|model| model.capabilities.as_ref())
        .is_some_and(|capabilities| {
            capabilities
                .iter()
                .any(|capability| matches!(capability.as_str(), "always_thinking" | "thinking"))
        });
    let (sender, receiver) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(sender)));
    let select_sender = Arc::clone(&sender);
    let cancel_sender = Arc::clone(&sender);
    let mut options = ModelSelectorOptions::new(
        models,
        first_alias,
        ThinkingEffort::from(if initial_thinking { "on" } else { "off" }),
        move |selection| send_result(&select_sender, Some(selection)),
        move || send_result(&cancel_sender, None),
    );
    options.searchable = true;
    await_prompt(
        host,
        Box::new(ModelSelectorComponent::new(options)),
        receiver,
    )
    .await
}

fn managed_model_alias(provider_id: &str, model: &ManagedKimiCodeModelInfo) -> ModelAlias {
    let protocol = (model.protocol == Some(ManagedKimiCodeProtocol::Anthropic))
        .then_some(ModelProtocol::Anthropic);
    let capabilities = model_capabilities(model);
    let adaptive_thinking = protocol.is_some()
        && capabilities.as_ref().is_some_and(|capabilities| {
            capabilities
                .iter()
                .any(|capability| matches!(capability.as_str(), "thinking" | "always_thinking"))
        });
    ModelAlias {
        provider: provider_id.to_owned(),
        model: model.id.clone(),
        max_context_size: model.context_length,
        max_output_size: None,
        capabilities,
        display_name: model.display_name.clone(),
        reasoning_key: None,
        protocol,
        adaptive_thinking: adaptive_thinking.then_some(true),
        support_efforts: model.support_efforts.clone(),
        default_effort: model.default_effort.clone(),
        beta_api: protocol.map(|_| true),
        overrides: None,
    }
}

fn catalog_model_alias(provider_id: &str, model: &CatalogModel) -> ModelAlias {
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
            capabilities.push(capability.to_owned());
        }
    }
    ModelAlias {
        provider: provider_id.to_owned(),
        model: model.id.clone(),
        max_context_size: model.capability.max_context_tokens,
        max_output_size: model.max_output_size.and_then(exact_u64),
        capabilities: (!capabilities.is_empty()).then_some(capabilities),
        display_name: model.name.clone(),
        reasoning_key: model.reasoning_key.clone(),
        protocol: None,
        adaptive_thinking: None,
        support_efforts: None,
        default_effort: None,
        beta_api: None,
        overrides: None,
    }
}

fn exact_u64(value: f64) -> Option<u64> {
    (value.is_finite() && value >= 0.0 && value <= u64::MAX as f64 && value.fract() == 0.0)
        .then_some(value as u64)
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use crate::cli::sub::provider::{CatalogCapability, CatalogModelEntry, CatalogProviderEntry};

    use super::*;

    #[derive(Default)]
    struct ImmediateHost {
        input: String,
        restored: usize,
        mounted: usize,
        errors: Vec<String>,
    }

    impl PromptHost for ImmediateHost {
        fn mount_editor_replacement(&mut self, mut component: Box<dyn Component>) {
            self.mounted += 1;
            component.handle_input(&self.input);
        }

        fn restore_editor(&mut self) {
            self.restored += 1;
        }

        fn show_error(&mut self, message: &str) {
            self.errors.push(message.to_owned());
        }
    }

    #[tokio::test]
    async fn platform_prompt_mounts_selects_and_restores_editor() {
        let mut host = ImmediateHost {
            input: "\r".to_owned(),
            ..ImmediateHost::default()
        };
        assert_eq!(
            prompt_platform_selection(&mut host).await.as_deref(),
            Some("kimi-code")
        );
        assert_eq!((host.mounted, host.restored), (1, 1));
    }

    #[tokio::test]
    async fn cancelled_prompt_restores_editor() {
        let mut host = ImmediateHost {
            input: "\u{1b}".to_owned(),
            ..ImmediateHost::default()
        };
        assert_eq!(prompt_feedback_attachment(&mut host).await, None);
        assert_eq!((host.mounted, host.restored), (1, 1));
    }

    #[tokio::test]
    async fn unsupported_catalog_reports_error_without_mounting() {
        let mut catalog = Catalog::new();
        catalog.insert(
            "broken".to_owned(),
            CatalogProviderEntry {
                id: None,
                name: Some("Broken".to_owned()),
                api: None,
                env: None,
                npm: None,
                provider_type: Some("unsupported".to_owned()),
                models: Some(IndexMap::<String, CatalogModelEntry>::new()),
                additional_fields: Map::new(),
            },
        );
        let mut host = ImmediateHost::default();
        assert_eq!(
            prompt_catalog_provider_selection(&mut host, &catalog).await,
            None
        );
        assert_eq!(host.mounted, 0);
        assert_eq!(
            host.errors,
            ["Catalog has no providers with supported wire types."]
        );
    }

    #[tokio::test]
    async fn model_selector_defaults_thinking_from_first_model_capabilities() {
        let mut models = IndexMap::new();
        models.insert(
            "provider/model".to_owned(),
            ModelAlias {
                provider: "provider".to_owned(),
                model: "model".to_owned(),
                max_context_size: 128_000,
                max_output_size: None,
                capabilities: Some(vec!["thinking".to_owned()]),
                display_name: None,
                reasoning_key: None,
                protocol: None,
                adaptive_thinking: None,
                support_efforts: None,
                default_effort: None,
                beta_api: None,
                overrides: None,
            },
        );
        let mut host = ImmediateHost {
            input: "\r".to_owned(),
            ..ImmediateHost::default()
        };
        let selection = run_model_selector(&mut host, models)
            .await
            .expect("selection");
        assert_eq!(selection.alias, "provider/model");
        assert_eq!(selection.thinking.as_str(), "on");
    }

    #[test]
    fn catalog_alias_preserves_selector_relevant_model_fields() {
        let model = CatalogModel {
            id: "chat".to_owned(),
            name: Some("Chat".to_owned()),
            max_output_size: Some(8192.0),
            reasoning_key: Some("reasoning_effort".to_owned()),
            capability: CatalogCapability {
                image_in: true,
                video_in: false,
                audio_in: false,
                thinking: true,
                tool_use: true,
                max_context_tokens: 200_000,
                dynamically_loaded_tools: false,
            },
        };
        let alias = catalog_model_alias("provider", &model);
        assert_eq!(alias.max_output_size, Some(8192));
        assert_eq!(alias.max_context_size, 200_000);
        assert_eq!(
            alias.capabilities,
            Some(vec![
                "image_in".to_owned(),
                "thinking".to_owned(),
                "tool_use".to_owned(),
            ])
        );
    }
}
