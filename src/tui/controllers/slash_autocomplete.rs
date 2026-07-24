use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::tui::{
    commands::registry::{
        BUILTIN_SLASH_COMMANDS, SlashArgumentCompletionKind, add_dir_argument_completions,
        goal_argument_completions, swarm_argument_completions,
    },
    components::editor::file_mention_provider::{
        AutocompleteItem, FileMentionProvider, InputMode, SlashAutocompleteCommand,
        SlashCommandMetadata,
    },
};

pub struct SlashAutocompleteSurface {
    pub provider: Arc<FileMentionProvider>,
    pub argument_hints: HashMap<String, String>,
    input_is_bash: Arc<AtomicBool>,
}

impl SlashAutocompleteSurface {
    pub fn set_input_mode(&self, mode: InputMode) {
        self.input_is_bash
            .store(mode == InputMode::Bash, Ordering::Relaxed);
    }

    pub fn input_mode(&self) -> InputMode {
        if self.input_is_bash.load(Ordering::Relaxed) {
            InputMode::Bash
        } else {
            InputMode::Prompt
        }
    }
}

// Original:
//   apps/kimi-code/src/tui/kimi-tui.ts
//   KimiTUI.refreshSlashCommandAutocomplete()
//
// Rust adaptation:
//   Dynamic skill and plugin commands will be added when their v2 catalogs are
//   composed. This initial surface installs the complete built-in registry,
//   aliases, descriptions, argument hints, and built-in argument completers.
pub fn build_builtin_slash_autocomplete(work_dir: PathBuf) -> SlashAutocompleteSurface {
    let input_is_bash = Arc::new(AtomicBool::new(false));
    let mode_reader = Arc::clone(&input_is_bash);
    let commands =
        BUILTIN_SLASH_COMMANDS
            .iter()
            .map(|command| {
                let metadata = SlashCommandMetadata {
                    name: command.name.to_owned(),
                    aliases: command
                        .aliases
                        .iter()
                        .map(|alias| (*alias).to_owned())
                        .collect(),
                    description: Some(command.description.to_owned()),
                    argument_hint: command.argument_hint.map(str::to_owned),
                };
                let autocomplete = SlashAutocompleteCommand::new(metadata);
                match command.completion {
                    Some(SlashArgumentCompletionKind::Goal) => autocomplete
                        .with_argument_completer(Arc::new(|prefix| {
                            goal_argument_completions(prefix).map(convert_items)
                        })),
                    Some(SlashArgumentCompletionKind::Swarm) => autocomplete
                        .with_argument_completer(Arc::new(|prefix| {
                            swarm_argument_completions(prefix).map(convert_items)
                        })),
                    Some(SlashArgumentCompletionKind::AddDir) => autocomplete
                        .with_argument_completer(Arc::new(|prefix| {
                            add_dir_argument_completions(prefix).map(convert_items)
                        })),
                    None => autocomplete,
                }
            })
            .collect();
    let argument_hints = BUILTIN_SLASH_COMMANDS
        .iter()
        .filter_map(|command| {
            command
                .argument_hint
                .map(|hint| (command.name.to_owned(), hint.to_owned()))
        })
        .collect();
    let provider = Arc::new(FileMentionProvider::new(
        commands,
        work_dir,
        None,
        Vec::new(),
        Arc::new(move || {
            if mode_reader.load(Ordering::Relaxed) {
                InputMode::Bash
            } else {
                InputMode::Prompt
            }
        }),
    ));

    SlashAutocompleteSurface {
        provider,
        argument_hints,
        input_is_bash,
    }
}

fn convert_items(
    items: Vec<crate::tui::commands::types::AutocompleteItem>,
) -> Vec<AutocompleteItem> {
    items
        .into_iter()
        .map(|item| AutocompleteItem {
            value: item.value,
            label: item.label,
            description: Some(item.description),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, atomic::AtomicBool};

    use super::*;

    #[tokio::test]
    async fn exposes_builtin_names_aliases_and_argument_completions() {
        let surface = build_builtin_slash_autocomplete(PathBuf::from("."));
        let cancelled = Arc::new(AtomicBool::new(false));

        let names = surface
            .provider
            .get_suggestions(&["/th".to_owned()], 0, 3, false, Arc::clone(&cancelled))
            .await
            .expect("name suggestions");
        assert!(names.items.iter().any(|item| item.value == "effort"));
        assert!(
            names
                .items
                .iter()
                .any(|item| item.label.contains("thinking"))
        );

        let arguments = surface
            .provider
            .get_suggestions(&["/goal re".to_owned()], 0, 8, false, cancelled)
            .await
            .expect("argument suggestions");
        assert!(arguments.items.iter().any(|item| item.value == "replace"));
        assert_eq!(
            surface.argument_hints.get("goal").map(String::as_str),
            Some("[status|pause|resume|cancel|replace|next] | <objective>")
        );
    }

    #[test]
    fn tracks_editor_input_mode_for_the_provider() {
        let surface = build_builtin_slash_autocomplete(PathBuf::from("."));
        surface.set_input_mode(InputMode::Bash);
        assert_eq!(surface.input_mode(), InputMode::Bash);
    }
}
