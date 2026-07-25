use std::sync::{Arc, Mutex, MutexGuard};

use indexmap::IndexMap;

use crate::{
    sdk::{
        model_alias::ModelAlias,
        types::{PermissionMode, ThinkingEffort},
    },
    tui::{
        commands::registry::BUILTIN_SLASH_COMMANDS,
        components::{
            Component,
            dialogs::{
                HelpPanelCommand, HelpPanelComponent, MigrationNoticeDialog,
                PermissionSelectorComponent, SettingsSelectorComponent, ThemeSelectorComponent,
                help_panel::HelpPanelOptions,
                model_selector::ModelSelection,
                tabbed_model_selector::{TabbedModelSelectorComponent, TabbedModelSelectorOptions},
            },
        },
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    Help,
    Settings,
    Permission,
    Theme,
    Model,
    MigrationNotice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogOutcome {
    Cancelled,
    Selected(String),
    ModelSelected(ModelSelection),
}

type SharedOutcome = Arc<Mutex<Option<DialogOutcome>>>;

pub struct MountedDialog {
    pub kind: DialogKind,
    component: Box<dyn Component>,
    outcome: SharedOutcome,
}

impl MountedDialog {
    pub fn render(&mut self, width: usize) -> Vec<String> {
        self.component.render(width)
    }

    pub fn handle_input(&mut self, data: &str) {
        self.component.handle_input(data);
    }

    pub fn take_outcome(&self) -> Option<DialogOutcome> {
        recover_lock(&self.outcome).take()
    }
}

fn recover_lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    match value.lock() {
        Ok(value) => value,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn set_outcome(outcome: &SharedOutcome, value: DialogOutcome) {
    *recover_lock(outcome) = Some(value);
}

fn mounted(
    kind: DialogKind,
    component: impl Component + 'static,
    outcome: SharedOutcome,
) -> MountedDialog {
    MountedDialog {
        kind,
        component: Box::new(component),
        outcome,
    }
}

// Original:
//   apps/kimi-code/src/tui/kimi-tui.ts
//   mountEditorReplacement(), restoreEditor(), showHelpPanel()
pub fn help_dialog() -> MountedDialog {
    let outcome = Arc::new(Mutex::new(None));
    let close_outcome = Arc::clone(&outcome);
    let commands = BUILTIN_SLASH_COMMANDS
        .iter()
        .map(|command| HelpPanelCommand {
            name: command.name.to_owned(),
            aliases: command
                .aliases
                .iter()
                .map(|alias| (*alias).to_owned())
                .collect(),
            description: command.description.to_owned(),
        })
        .collect();
    let mut panel = HelpPanelComponent::new(HelpPanelOptions::new(commands, move || {
        set_outcome(&close_outcome, DialogOutcome::Cancelled);
    }));
    panel.focused = true;
    mounted(DialogKind::Help, panel, outcome)
}

pub fn settings_dialog() -> MountedDialog {
    let outcome = Arc::new(Mutex::new(None));
    let select_outcome = Arc::clone(&outcome);
    let cancel_outcome = Arc::clone(&outcome);
    let selector = SettingsSelectorComponent::new(
        move |selection| {
            set_outcome(
                &select_outcome,
                DialogOutcome::Selected(format!(
                    "Settings selection '{}' received; its v2 backend is pending.",
                    selection.as_str()
                )),
            );
        },
        move || set_outcome(&cancel_outcome, DialogOutcome::Cancelled),
    );
    mounted(DialogKind::Settings, selector, outcome)
}

pub fn permission_dialog() -> MountedDialog {
    let outcome = Arc::new(Mutex::new(None));
    let select_outcome = Arc::clone(&outcome);
    let cancel_outcome = Arc::clone(&outcome);
    let selector = PermissionSelectorComponent::new(
        PermissionMode::Manual,
        move |mode| {
            let mode = match mode {
                PermissionMode::Manual => "manual",
                PermissionMode::Yolo => "yolo",
                PermissionMode::Auto => "auto",
            };
            set_outcome(
                &select_outcome,
                DialogOutcome::Selected(format!(
                    "Permission mode '{mode}' selected; v2 session update is pending."
                )),
            );
        },
        move || set_outcome(&cancel_outcome, DialogOutcome::Cancelled),
    );
    mounted(DialogKind::Permission, selector, outcome)
}

pub fn theme_dialog() -> MountedDialog {
    let outcome = Arc::new(Mutex::new(None));
    let select_outcome = Arc::clone(&outcome);
    let cancel_outcome = Arc::clone(&outcome);
    let selector = ThemeSelectorComponent::new_with_custom_themes(
        "auto",
        Vec::new(),
        move |theme| {
            set_outcome(
                &select_outcome,
                DialogOutcome::Selected(format!(
                    "Theme '{theme}' selected; persistence is pending."
                )),
            );
        },
        move || set_outcome(&cancel_outcome, DialogOutcome::Cancelled),
    );
    mounted(DialogKind::Theme, selector, outcome)
}

/// Mounts the same searchable, provider-tabbed selector used by the source
/// `/model` command. The async config/runtime update is deliberately handled
/// by the coordinator after this typed outcome is emitted.
pub fn model_dialog(
    models: IndexMap<String, ModelAlias>,
    current_value: impl Into<String>,
    selected_value: Option<String>,
    current_thinking_effort: ThinkingEffort,
    warning: Option<String>,
) -> MountedDialog {
    let current_value = current_value.into();
    let outcome = Arc::new(Mutex::new(None));
    let select_outcome = Arc::clone(&outcome);
    let cancel_outcome = Arc::clone(&outcome);
    let mut options = TabbedModelSelectorOptions::new(
        models,
        current_value,
        current_thinking_effort,
        move |selection| {
            set_outcome(&select_outcome, DialogOutcome::ModelSelected(selection));
        },
        move || set_outcome(&cancel_outcome, DialogOutcome::Cancelled),
    );
    options.selected_value = selected_value;
    options.warning = warning;
    let mut selector = TabbedModelSelectorComponent::new(options);
    selector.focused = true;
    mounted(DialogKind::Model, selector, outcome)
}

pub fn migration_notice_dialog(command_name: &str, args: &str) -> MountedDialog {
    let outcome = Arc::new(Mutex::new(None));
    let close_outcome = Arc::clone(&outcome);
    let arguments = if args.is_empty() {
        String::new()
    } else {
        format!(" Arguments received: {args}")
    };
    let dialog = MigrationNoticeDialog::new(
        format!("/{command_name}"),
        format!("This operation is available, but its v2 backend is not connected yet.{arguments}"),
        move || set_outcome(&close_outcome, DialogOutcome::Cancelled),
    );
    mounted(DialogKind::MigrationNotice, dialog, outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_and_notice_dialogs_emit_close_outcomes() {
        let mut help = help_dialog();
        assert_eq!(help.kind, DialogKind::Help);
        assert!(help.render(80).join("\n").contains("Keyboard shortcuts"));
        help.handle_input("\u{1b}");
        assert_eq!(help.take_outcome(), Some(DialogOutcome::Cancelled));

        let mut notice = migration_notice_dialog("model", "fast");
        assert!(
            notice
                .render(120)
                .join("\n")
                .contains("Arguments received: fast")
        );
        notice.handle_input("\r");
        assert_eq!(notice.take_outcome(), Some(DialogOutcome::Cancelled));
    }

    #[test]
    fn selectors_return_typed_visible_defaults() {
        let mut permission = permission_dialog();
        permission.handle_input("\u{1b}[B");
        permission.handle_input("\r");
        assert_eq!(
            permission.take_outcome(),
            Some(DialogOutcome::Selected(
                "Permission mode 'yolo' selected; v2 session update is pending.".to_owned()
            ))
        );

        let mut settings = settings_dialog();
        settings.handle_input("\r");
        assert_eq!(
            settings.take_outcome(),
            Some(DialogOutcome::Selected(
                "Settings selection 'model' received; its v2 backend is pending.".to_owned()
            ))
        );
    }
}
