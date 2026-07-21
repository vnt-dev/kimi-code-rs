use async_trait::async_trait;

use super::types::{InstallSource, UpdateTarget};

pub const CHANGELOG_URL: &str =
    "https://moonshotai.github.io/kimi-code/en/release-notes/changelog.html";
pub const HIDE_CURSOR: &str = "\u{1b}[?25l";
pub const SHOW_CURSOR: &str = "\u{1b}[?25h";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPromptChoiceValue {
    Install,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPromptChoice {
    pub value: InstallPromptChoiceValue,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKey {
    Up,
    Down,
    Enter,
    Escape,
    CtrlC,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPromptOptions {
    pub current_version: String,
    pub target: UpdateTarget,
    pub install_command: String,
    pub install_source: InstallSource,
}

#[async_trait]
pub trait InstallPromptRuntime: Send + Sync {
    fn raw_mode(&self) -> bool;

    fn can_set_raw_mode(&self) -> bool;

    fn set_raw_mode(&self, enabled: bool);

    fn resume_input(&self);

    async fn next_keypress(&self) -> PromptKey;

    fn color_enabled(&self) -> bool;

    fn write_output(&self, text: &str);
}

// Original:
//   apps/kimi-code/src/cli/update/prompt.ts
//   createInstallPromptChoices()
pub fn create_install_prompt_choices(target: &UpdateTarget) -> Vec<InstallPromptChoice> {
    vec![
        InstallPromptChoice {
            value: InstallPromptChoiceValue::Install,
            label: format!("Install update now ({})", target.version),
        },
        InstallPromptChoice {
            value: InstallPromptChoiceValue::Skip,
            label: "Continue with current version".to_owned(),
        },
    ]
}

pub fn get_default_install_prompt_selection(choices: &[InstallPromptChoice]) -> usize {
    choices
        .iter()
        .position(|choice| choice.value == InstallPromptChoiceValue::Install)
        .unwrap_or(0)
}

// Original: moveInstallPromptSelection()
pub fn move_install_prompt_selection(
    current_index: i32,
    direction: PromptDirection,
    choice_count: i32,
) -> i32 {
    match direction {
        PromptDirection::Up => 0.max(current_index - 1),
        PromptDirection::Down => (choice_count - 1).min(current_index + 1),
    }
}

// Original: promptForInstallChoice()
pub async fn prompt_for_install_choice(
    runtime: &dyn InstallPromptRuntime,
    options: &InstallPromptOptions,
) -> InstallPromptChoiceValue {
    let choices = create_install_prompt_choices(&options.target);
    let mut selected_index = get_default_install_prompt_selection(&choices);
    let mut line_count = 0;
    let had_raw_mode = runtime.raw_mode();
    if runtime.can_set_raw_mode() {
        runtime.set_raw_mode(true);
    }
    runtime.resume_input();
    runtime.write_output(HIDE_CURSOR);
    render_frame(runtime, options, &choices, selected_index, &mut line_count);

    let choice = loop {
        match runtime.next_keypress().await {
            PromptKey::Up => {
                selected_index = usize::try_from(move_install_prompt_selection(
                    selected_index as i32,
                    PromptDirection::Up,
                    choices.len() as i32,
                ))
                .unwrap_or(0);
                render_frame(runtime, options, &choices, selected_index, &mut line_count);
            }
            PromptKey::Down => {
                selected_index = usize::try_from(move_install_prompt_selection(
                    selected_index as i32,
                    PromptDirection::Down,
                    choices.len() as i32,
                ))
                .unwrap_or(0);
                render_frame(runtime, options, &choices, selected_index, &mut line_count);
            }
            PromptKey::Enter => {
                break choices
                    .get(selected_index)
                    .map_or(InstallPromptChoiceValue::Skip, |choice| choice.value);
            }
            PromptKey::Escape | PromptKey::CtrlC => break InstallPromptChoiceValue::Skip,
            PromptKey::Other => {}
        }
    };

    if runtime.can_set_raw_mode() {
        runtime.set_raw_mode(had_raw_mode);
    }
    runtime.write_output(SHOW_CURSOR);
    runtime.write_output("\n");
    choice
}

fn render_frame(
    runtime: &dyn InstallPromptRuntime,
    options: &InstallPromptOptions,
    choices: &[InstallPromptChoice],
    selected_index: usize,
    previous_line_count: &mut usize,
) {
    let lines = render_install_prompt(options, choices, selected_index, runtime.color_enabled());
    if *previous_line_count > 0 {
        runtime.write_output(&format!("\u{1b}[{}A", *previous_line_count - 1));
    }
    for (index, line) in lines.iter().enumerate() {
        runtime.write_output("\u{1b}[2K\r");
        runtime.write_output(line);
        if index + 1 < lines.len() {
            runtime.write_output("\n");
        }
    }
    *previous_line_count = lines.len();
}

fn render_install_prompt(
    options: &InstallPromptOptions,
    choices: &[InstallPromptChoice],
    selected_index: usize,
    color: bool,
) -> Vec<String> {
    let style = PromptStyle { color };
    let changelog_text = style.primary_underline(&format!("View changelog: {CHANGELOG_URL}"));
    let mut lines = vec![
        style.primary_bold("Kimi Code Update Available"),
        style.muted("Kimi Code has a newer release ready."),
        format!("\u{1b}]8;;{CHANGELOG_URL}\u{1b}\\{changelog_text}\u{1b}]8;;\u{1b}\\"),
        String::new(),
        format!(
            "{}  {}",
            style.label("Current"),
            style.warning_bold(&options.current_version)
        ),
        format!(
            "{}  {}",
            style.label("Target "),
            style.success_bold(&options.target.version)
        ),
        format!(
            "{}  {}",
            style.label("Source "),
            style.primary_bold(options.install_source.as_str())
        ),
        format!(
            "{}  {}",
            style.label("Command"),
            style.primary(&options.install_command)
        ),
        String::new(),
        style.muted("↑↓ choose · Enter confirm · Esc continue"),
        String::new(),
    ];
    for (index, choice) in choices.iter().enumerate() {
        let selected = index == selected_index;
        let pointer = if selected { '❯' } else { ' ' };
        let content = format!(" {pointer} {}", choice.label);
        lines.push(if selected {
            style.primary_bold(&content)
        } else {
            style.label(&content)
        });
    }
    lines
}

struct PromptStyle {
    color: bool,
}

impl PromptStyle {
    fn primary(&self, text: &str) -> String {
        self.paint(text, (23, 131, 255), false, false)
    }
    fn primary_bold(&self, text: &str) -> String {
        self.paint(text, (23, 131, 255), true, false)
    }
    fn primary_underline(&self, text: &str) -> String {
        self.paint(text, (23, 131, 255), false, true)
    }
    fn success_bold(&self, text: &str) -> String {
        self.paint(text, (22, 163, 74), true, false)
    }
    fn warning_bold(&self, text: &str) -> String {
        self.paint(text, (202, 138, 4), true, false)
    }
    fn muted(&self, text: &str) -> String {
        self.paint(text, (153, 153, 153), false, false)
    }
    fn label(&self, text: &str) -> String {
        self.paint(text, (107, 114, 128), true, false)
    }

    fn paint(
        &self,
        text: &str,
        (red, green, blue): (u8, u8, u8),
        bold: bool,
        underline: bool,
    ) -> String {
        if !self.color {
            return text.to_owned();
        }
        let mut codes = Vec::new();
        if bold {
            codes.push("1".to_owned());
        }
        if underline {
            codes.push("4".to_owned());
        }
        codes.push(format!("38;2;{red};{green};{blue}"));
        format!("\u{1b}[{}m{text}\u{1b}[0m", codes.join(";"))
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use super::*;

    struct RuntimeMock {
        raw: bool,
        can_set_raw: bool,
        color: bool,
        keys: Mutex<VecDeque<PromptKey>>,
        raw_changes: Mutex<Vec<bool>>,
        resumed: Mutex<usize>,
        output: Mutex<String>,
    }

    impl RuntimeMock {
        fn with_keys(keys: impl IntoIterator<Item = PromptKey>) -> Self {
            Self {
                raw: false,
                can_set_raw: true,
                color: false,
                keys: Mutex::new(keys.into_iter().collect()),
                raw_changes: Mutex::new(Vec::new()),
                resumed: Mutex::new(0),
                output: Mutex::new(String::new()),
            }
        }
    }

    #[async_trait]
    impl InstallPromptRuntime for RuntimeMock {
        fn raw_mode(&self) -> bool {
            self.raw
        }
        fn can_set_raw_mode(&self) -> bool {
            self.can_set_raw
        }
        fn set_raw_mode(&self, enabled: bool) {
            self.raw_changes.lock().expect("raw").push(enabled);
        }
        fn resume_input(&self) {
            *self.resumed.lock().expect("resumed") += 1;
        }
        async fn next_keypress(&self) -> PromptKey {
            self.keys.lock().expect("keys").pop_front().expect("key")
        }
        fn color_enabled(&self) -> bool {
            self.color
        }
        fn write_output(&self, text: &str) {
            self.output.lock().expect("output").push_str(text);
        }
    }

    fn options() -> InstallPromptOptions {
        InstallPromptOptions {
            current_version: "0.4.0".to_owned(),
            target: UpdateTarget {
                version: "0.5.0".to_owned(),
            },
            install_command: "npm install -g @moonshot-ai/kimi-code@0.5.0".to_owned(),
            install_source: InstallSource::NpmGlobal,
        }
    }

    #[test]
    fn choices_default_to_install_and_selection_clamps() {
        let choices = create_install_prompt_choices(&options().target);
        assert_eq!(get_default_install_prompt_selection(&choices), 0);
        assert_eq!(choices[0].label, "Install update now (0.5.0)");
        assert_eq!(choices[1].label, "Continue with current version");
        assert_eq!(move_install_prompt_selection(1, PromptDirection::Up, 2), 0);
        assert_eq!(move_install_prompt_selection(0, PromptDirection::Up, 2), 0);
        assert_eq!(
            move_install_prompt_selection(0, PromptDirection::Down, 2),
            1
        );
        assert_eq!(
            move_install_prompt_selection(1, PromptDirection::Down, 2),
            1
        );
    }

    #[tokio::test]
    async fn renders_hyperlink_and_escape_skips_then_restores_terminal() {
        let runtime = RuntimeMock::with_keys([PromptKey::Escape]);
        let choice = prompt_for_install_choice(&runtime, &options()).await;
        assert_eq!(choice, InstallPromptChoiceValue::Skip);
        let output = runtime.output.lock().expect("output");
        assert!(output.contains(CHANGELOG_URL));
        assert!(output.contains("View changelog"));
        assert!(output.starts_with(HIDE_CURSOR));
        assert!(output.ends_with(&format!("{SHOW_CURSOR}\n")));
        assert_eq!(
            runtime.raw_changes.lock().expect("raw").as_slice(),
            [true, false]
        );
    }

    #[tokio::test]
    async fn down_then_enter_selects_skip_and_repaints_in_place() {
        let runtime = RuntimeMock::with_keys([PromptKey::Down, PromptKey::Enter]);
        let choice = prompt_for_install_choice(&runtime, &options()).await;
        assert_eq!(choice, InstallPromptChoiceValue::Skip);
        let output = runtime.output.lock().expect("output");
        assert!(output.contains("\u{1b}[12A"));
        assert!(output.contains(" ❯ Continue with current version"));
    }

    #[tokio::test]
    async fn enter_accepts_default_install_and_ctrl_c_skips() {
        let install_runtime = RuntimeMock::with_keys([PromptKey::Enter]);
        assert_eq!(
            prompt_for_install_choice(&install_runtime, &options()).await,
            InstallPromptChoiceValue::Install
        );
        let skip_runtime = RuntimeMock::with_keys([PromptKey::CtrlC]);
        assert_eq!(
            prompt_for_install_choice(&skip_runtime, &options()).await,
            InstallPromptChoiceValue::Skip
        );
    }
}
