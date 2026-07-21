use std::any::Any;

use crate::tui::{
    components::{
        Component, ComponentRole, Input, InputAction,
        media::{
            code_highlight::{highlight_lines, lang_from_path},
            diff_preview::{ClusteredDiffOptions, render_diff_lines_clustered},
        },
        render::{truncate_to_width, visible_width, wrap_text_with_ansi},
    },
    keys::{EditorKey, matches_editor_key},
    reverse_rpc::{
        ApprovalDecision, ApprovalPanelChoice, DisplayBlock, FileContentDisplayBlock,
        PendingApproval,
    },
    theme::{ColorToken, current_theme},
    utils::printable_key::printable_char,
};

use super::ApprovalPreviewBlock;

const DIFF_SUMMARY_MAX_LINES: usize = 10;
const CONTENT_SUMMARY_MAX_LINES: usize = 10;

type ResponseCallback = dyn FnMut(ApprovalPanelResponse) + Send;
type ToggleCallback = dyn FnMut() + Send;
type PreviewCallback = dyn FnMut(ApprovalPreviewBlock) + Send;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPanelResponse {
    pub response: ApprovalDecision,
    pub feedback: Option<String>,
    pub selected_label: Option<String>,
}

/// Keyboard-driven approval request panel.
///
/// Original: `approval-panel.ts`, `ApprovalPanelComponent`.
pub struct ApprovalPanelComponent {
    pub focused: bool,
    selected_index: usize,
    feedback_mode: bool,
    feedback_input: Input,
    on_response: Box<ResponseCallback>,
    request: PendingApproval,
    on_toggle_tool_output: Option<Box<ToggleCallback>>,
    on_open_preview: Option<Box<PreviewCallback>>,
}

impl ApprovalPanelComponent {
    pub fn new<F>(request: PendingApproval, on_response: F) -> Self
    where
        F: FnMut(ApprovalPanelResponse) + Send + 'static,
    {
        Self::new_with_callbacks(request, on_response, None, None)
    }

    pub fn new_with_callbacks<F>(
        request: PendingApproval,
        on_response: F,
        on_toggle_tool_output: Option<Box<ToggleCallback>>,
        on_open_preview: Option<Box<PreviewCallback>>,
    ) -> Self
    where
        F: FnMut(ApprovalPanelResponse) + Send + 'static,
    {
        Self {
            focused: false,
            selected_index: 0,
            feedback_mode: false,
            feedback_input: Input::new(),
            on_response: Box::new(on_response),
            request,
            on_toggle_tool_output,
            on_open_preview,
        }
    }

    /// Original: `ApprovalPanelComponent.submit()`.
    fn submit(&mut self, index: usize, feedback: &str) {
        let Some(option) = self.choice_at(index).cloned() else {
            return;
        };
        (self.on_response)(ApprovalPanelResponse {
            response: option.response,
            feedback: (!feedback.is_empty()).then(|| feedback.to_owned()),
            selected_label: option.selected_label,
        });
    }

    /// Original: `ApprovalPanelComponent.selectAndSubmit()`.
    fn select_and_submit(&mut self, index: usize) {
        let Some(option) = self.choice_at(index) else {
            return;
        };
        if option.requires_feedback {
            self.selected_index = index;
            self.feedback_mode = true;
        } else {
            self.submit(index, "");
        }
    }

    /// Original: `ApprovalPanelComponent.handleInput()`.
    pub fn handle_input_event(&mut self, data: &str) {
        if matches_editor_key(data, EditorKey::Escape)
            || matches_editor_key(data, EditorKey::Ctrl('c'))
            || matches_editor_key(data, EditorKey::Ctrl('d'))
        {
            (self.on_response)(ApprovalPanelResponse {
                response: ApprovalDecision::Rejected,
                feedback: None,
                selected_label: None,
            });
            return;
        }
        if matches_editor_key(data, EditorKey::Ctrl('e')) {
            if let Some(block) = self.find_previewable_block()
                && let Some(callback) = &mut self.on_open_preview
            {
                callback(block);
            }
            return;
        }
        if matches_editor_key(data, EditorKey::Ctrl('o')) {
            if let Some(callback) = &mut self.on_toggle_tool_output {
                callback();
            }
            return;
        }
        if self.feedback_mode {
            let count = self.choice_count();
            if matches_editor_key(data, EditorKey::Up) {
                self.feedback_mode = false;
                if count > 0 {
                    self.selected_index = (self.selected_index + count - 1) % count;
                }
                return;
            }
            if matches_editor_key(data, EditorKey::Down) {
                self.feedback_mode = false;
                if count > 0 {
                    self.selected_index = (self.selected_index + 1) % count;
                }
                return;
            }
            if let Some(InputAction::Submit(value)) = self.feedback_input.handle_input_event(data) {
                self.submit(self.selected_index, &value);
            }
            return;
        }
        let count = self.choice_count();
        if count == 0 {
            return;
        }
        if matches_editor_key(data, EditorKey::Up) {
            self.selected_index = (self.selected_index + count - 1) % count;
            return;
        }
        if matches_editor_key(data, EditorKey::Down) {
            self.selected_index = (self.selected_index + 1) % count;
            return;
        }
        if matches_editor_key(data, EditorKey::Enter) {
            self.select_and_submit(self.selected_index);
            return;
        }
        if let Some(index) = printable_char(data)
            .parse::<usize>()
            .ok()
            .and_then(|number| number.checked_sub(1))
            .filter(|index| *index < count)
        {
            self.select_and_submit(index);
        }
    }

    /// Original: `ApprovalPanelComponent.render()`.
    pub fn render_panel(&mut self, width: usize) -> Vec<String> {
        self.ensure_valid_selection();
        self.feedback_input.focused = self.focused && self.feedback_mode;
        let theme = current_theme();
        let horizontal = theme.fg(ColorToken::BorderFocus, &"─".repeat(width));
        let title = header_for(&self.request.data.tool_name);
        let mut lines = vec![
            horizontal.clone(),
            format!(
                "  {} {}",
                theme.bold_fg(ColorToken::BorderFocus, "◆"),
                theme.bold_fg(ColorToken::BorderFocus, &title)
            ),
        ];

        let visible_blocks = self
            .request
            .data
            .display
            .iter()
            .filter(|block| !is_duplicate_brief_block(block, &self.request.data.description))
            .take(5)
            .collect::<Vec<_>>();
        let has_previewable = visible_blocks
            .iter()
            .any(|block| matches!(block, DisplayBlock::Diff(_) | DisplayBlock::FileContent(_)));
        if !visible_blocks.is_empty() {
            lines.push(String::new());
            for block in visible_blocks {
                for line in render_display_block(block, width.saturating_sub(2).max(1)) {
                    lines.push(format!("  {line}"));
                }
            }
        } else if !self.request.data.description.is_empty() {
            lines.push(String::new());
            for description in self.request.data.description.split('\n') {
                lines.push(format!("  {}", theme.fg(ColorToken::TextDim, description)));
            }
        }

        lines.push(String::new());
        for (index, option) in self.request.data.choices.iter().enumerate() {
            let selected = index == self.selected_index;
            let label = format!("{}. {}", index + 1, option.label);
            if self.feedback_mode && option.requires_feedback && selected {
                lines.push(format!(
                    "  {}",
                    self.render_inline_feedback_line(width.saturating_sub(2), &label)
                ));
            } else if selected {
                lines.push(format!(
                    "  {} {}",
                    theme.bold_fg(ColorToken::Accent, "◆"),
                    theme.bold_fg(ColorToken::Accent, &label)
                ));
            } else {
                lines.push(format!(
                    "  {}",
                    theme.fg(ColorToken::TextStrong, &format!("  {label}"))
                ));
            }
            if !(self.feedback_mode && option.requires_feedback && selected)
                && let Some(description) = option
                    .description
                    .as_deref()
                    .filter(|text| !text.is_empty())
            {
                for line in wrap_text_with_ansi(description, width.saturating_sub(7).max(20)) {
                    lines.push(format!("       {}", theme.fg(ColorToken::TextDim, &line)));
                }
            }
        }
        lines.push(String::new());
        if self.feedback_mode {
            lines.push(format!(
                "  {}",
                theme.fg(ColorToken::TextDim, "Type feedback · ↵ submit.")
            ));
        } else {
            let preview_hint = if has_previewable {
                " · ctrl+e preview"
            } else {
                ""
            };
            lines.push(format!(
                "  {}",
                theme.fg(
                    ColorToken::TextDim,
                    &format!(
                        "↑/↓ select · {} choose · ↵ confirm{preview_hint}",
                        build_numeric_hint(self.choice_count())
                    )
                )
            ));
        }
        lines.push(horizontal);
        lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "...", false))
            .collect()
    }

    /// Original: `ApprovalPanelComponent.findPreviewableBlock()`.
    fn find_previewable_block(&self) -> Option<ApprovalPreviewBlock> {
        self.request
            .data
            .display
            .iter()
            .find_map(|block| match block {
                DisplayBlock::Diff(block) => Some(ApprovalPreviewBlock::Diff(block.clone())),
                DisplayBlock::FileContent(block) => {
                    Some(ApprovalPreviewBlock::FileContent(block.clone()))
                }
                _ => None,
            })
    }

    fn choice_at(&self, index: usize) -> Option<&ApprovalPanelChoice> {
        self.request.data.choices.get(index)
    }

    fn choice_count(&self) -> usize {
        self.request.data.choices.len()
    }

    /// Original: `ApprovalPanelComponent.ensureValidSelection()`.
    fn ensure_valid_selection(&mut self) {
        self.selected_index = self
            .selected_index
            .min(self.choice_count().saturating_sub(1));
    }

    /// Original: `ApprovalPanelComponent.renderInlineFeedbackLine()`.
    fn render_inline_feedback_line(&self, width: usize, label: &str) -> String {
        let theme = current_theme();
        let prefix = format!(
            "{} {}  ",
            theme.bold_fg(ColorToken::Accent, "◆"),
            theme.bold_fg(ColorToken::Accent, label)
        );
        let input_width = width
            .saturating_sub(visible_width(&prefix))
            .saturating_add(2)
            .max(4);
        let input = self.feedback_input.render_line(input_width);
        format!("{prefix}{}", input.strip_prefix("> ").unwrap_or(&input))
    }
}

impl Component for ApprovalPanelComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_panel(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.handle_input_event(data);
    }

    fn wants_key_release(&self) -> bool {
        true
    }

    fn invalidate(&mut self) {
        self.feedback_input.invalidate();
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Original: `approval-panel.ts`, `truncateOneLine()`.
fn truncate_one_line(text: &str, max: usize) -> String {
    let first = text.lines().next().unwrap_or_default();
    if first.encode_utf16().count() <= max {
        return first.to_owned();
    }
    let keep = max.saturating_sub(1);
    let mut used = 0;
    let mut output = String::new();
    for character in first.chars() {
        let units = character.len_utf16();
        if used + units > keep {
            break;
        }
        output.push(character);
        used += units;
    }
    output.push('…');
    output
}

fn append_wrapped_line(
    lines: &mut Vec<String>,
    first_prefix: &str,
    continuation_prefix: &str,
    content: &str,
    width: usize,
) {
    let prefix_width = visible_width(first_prefix).max(visible_width(continuation_prefix));
    let wrapped = wrap_text_with_ansi(content, width.saturating_sub(prefix_width).max(1));
    if wrapped.is_empty() {
        lines.push(first_prefix.to_owned());
        return;
    }
    lines.push(format!("{first_prefix}{}", wrapped[0]));
    for line in wrapped.iter().skip(1) {
        lines.push(format!("{continuation_prefix}{line}"));
    }
}

/// Original: `approval-panel.ts`, `renderShellDisplayBlock()`.
fn render_shell_display_block(
    command: &str,
    cwd: Option<&str>,
    description: Option<&str>,
    danger: Option<&str>,
    width: usize,
) -> Vec<String> {
    let theme = current_theme();
    let mut lines = Vec::new();
    if let Some(cwd) = cwd.filter(|text| !text.is_empty()) {
        lines.push(theme.fg(ColorToken::TextDim, &format!("cwd: {cwd}")));
    }
    if let Some(danger) = danger {
        lines.push(theme.bold_fg(ColorToken::Error, &format!("Dangerous: {danger}")));
    }
    let commands = if command.is_empty() {
        vec![""]
    } else {
        command.split('\n').collect()
    };
    for (index, line) in commands.iter().enumerate() {
        let prefix = if index == 0 {
            format!("{} ", theme.fg(ColorToken::Accent, "$"))
        } else {
            format!("{} ", theme.fg(ColorToken::TextDim, "·"))
        };
        append_wrapped_line(
            &mut lines,
            &prefix,
            "  ",
            &theme.fg(ColorToken::TextStrong, line),
            width,
        );
    }
    if let Some(description) = description.filter(|text| !text.is_empty()) {
        lines.push(format!("  {}", theme.fg(ColorToken::TextDim, description)));
    }
    lines
}

/// Original: `approval-panel.ts`, `renderDisplayBlock()`.
fn render_display_block(block: &DisplayBlock, width: usize) -> Vec<String> {
    let theme = current_theme();
    match block {
        DisplayBlock::Diff(block) => render_diff_lines_clustered(
            &block.old_text,
            &block.new_text,
            &block.path,
            &ClusteredDiffOptions {
                context_lines: Some(3),
                expand_key_hint: Some("ctrl+e to preview".to_owned()),
                max_lines: Some(DIFF_SUMMARY_MAX_LINES),
                ..ClusteredDiffOptions::default()
            },
        ),
        DisplayBlock::FileContent(block) => render_file_content(block),
        DisplayBlock::Shell {
            command,
            cwd,
            description,
            danger,
            ..
        } => render_shell_display_block(
            command,
            cwd.as_deref(),
            description.as_deref(),
            danger.as_deref(),
            width,
        ),
        DisplayBlock::FileOp {
            operation,
            path,
            detail,
        } => {
            let mut lines = vec![format!(
                "{} {}",
                theme.fg(ColorToken::Accent, &format!("{:<5}", operation.as_str())),
                theme.fg(ColorToken::TextStrong, path)
            )];
            if let Some(detail) = detail.as_deref().filter(|text| !text.is_empty()) {
                lines.push(theme.fg(ColorToken::TextDim, detail));
            }
            lines
        }
        DisplayBlock::UrlFetch { url, method } => vec![format!(
            "{} {}",
            theme.fg(
                ColorToken::Accent,
                &format!("{:<5}", method.as_deref().unwrap_or("GET").to_uppercase())
            ),
            theme.fg(ColorToken::TextStrong, url)
        )],
        DisplayBlock::Search { query, scope } => {
            let mut lines = vec![format!(
                "{} {}",
                theme.fg(ColorToken::Accent, "search"),
                theme.fg(ColorToken::TextStrong, query)
            )];
            if let Some(scope) = scope.as_deref().filter(|text| !text.is_empty()) {
                lines.push(theme.fg(ColorToken::TextDim, &format!("scope: {scope}")));
            }
            lines
        }
        DisplayBlock::Invocation {
            kind,
            name,
            description,
        } => {
            let mut lines = vec![format!(
                "{} {}",
                theme.fg(ColorToken::Accent, &format!("{:<5}", kind.as_str())),
                theme.fg(ColorToken::TextStrong, name)
            )];
            if let Some(description) = description.as_deref().filter(|text| !text.is_empty()) {
                lines.push(theme.fg(ColorToken::TextDim, &truncate_one_line(description, 200)));
            }
            lines
        }
        DisplayBlock::Brief { text } if !text.is_empty() => text
            .split('\n')
            .map(|line| {
                if line.is_empty() {
                    String::new()
                } else {
                    theme.fg(ColorToken::TextStrong, line)
                }
            })
            .collect(),
        DisplayBlock::Brief { .. } => Vec::new(),
        DisplayBlock::BackgroundTask {
            task_id,
            kind,
            status,
            description,
        } => vec![theme.fg(
            ColorToken::TextStrong,
            &format!("{status} {kind} task {task_id}: {description}"),
        )],
        DisplayBlock::Todo { items } => items
            .iter()
            .map(|item| {
                theme.fg(
                    ColorToken::TextStrong,
                    &format!("- [{}] {}", item.status.as_str(), item.title),
                )
            })
            .collect(),
    }
}

fn render_file_content(block: &FileContentDisplayBlock) -> Vec<String> {
    let inferred;
    let language = if let Some(language) = block.language.as_deref() {
        Some(language)
    } else {
        inferred = lang_from_path(&block.path);
        inferred.as_deref()
    };
    let all = highlight_lines(&block.content, language);
    let shown = all.len().min(CONTENT_SUMMARY_MAX_LINES);
    let mut lines = vec![current_theme().fg(ColorToken::TextStrong, &block.path)];
    for (index, line) in all.iter().take(shown).enumerate() {
        lines.push(format!(
            "{}{}",
            current_theme().fg(ColorToken::DiffGutter, &format!("{:>4}  ", index + 1)),
            line
        ));
    }
    let remaining = all.len() - shown;
    if remaining > 0 {
        lines.push(current_theme().fg(
            ColorToken::TextDim,
            &format!(
                "     … {remaining} more line{} hidden (ctrl+e to preview)",
                if remaining > 1 { "s" } else { "" }
            ),
        ));
    }
    lines
}

/// Original: `approval-panel.ts`, `normalizeApprovalText()`.
fn normalize_approval_text(text: &str) -> String {
    text.replace("\r\n", "\n").trim().to_owned()
}

/// Original: `approval-panel.ts`, `isDuplicateBriefBlock()`.
fn is_duplicate_brief_block(block: &DisplayBlock, description: &str) -> bool {
    let DisplayBlock::Brief { text } = block else {
        return false;
    };
    if text.trim().is_empty() {
        return false;
    }
    let description = normalize_approval_text(description);
    if description.is_empty() {
        return false;
    }
    let block = normalize_approval_text(text);
    if block == description {
        return true;
    }
    let lines = block.split('\n').collect::<Vec<_>>();
    lines.len() > 1 && normalize_approval_text(&lines[1..].join("\n")) == description
}

/// Original: `approval-panel.ts`, `headerFor()`.
fn header_for(tool_name: &str) -> String {
    match tool_name {
        "Bash" => "Run this command?".to_owned(),
        "Write" => "Write this file?".to_owned(),
        "Edit" => "Apply these edits?".to_owned(),
        "TaskStop" => "Stop this task?".to_owned(),
        "ExitPlanMode" => "Ready to build with this plan?".to_owned(),
        other => format!("Approve {other}?"),
    }
}

/// Original: `approval-panel.ts`, `buildNumericHint()`.
fn build_numeric_hint(count: usize) -> String {
    if count == 0 {
        return "–".to_owned();
    }
    (1..=count.min(9))
        .map(|number| number.to_string())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::tui::reverse_rpc::{ApprovalPanelData, DiffDisplayBlock};

    use super::*;

    fn choice(label: &str, response: ApprovalDecision) -> ApprovalPanelChoice {
        ApprovalPanelChoice {
            label: label.to_owned(),
            response,
            selected_label: None,
            requires_feedback: false,
            description: None,
        }
    }

    fn pending() -> PendingApproval {
        PendingApproval {
            data: ApprovalPanelData {
                id: "approval_1".to_owned(),
                tool_call_id: "tool_1".to_owned(),
                tool_name: "WriteFile".to_owned(),
                action: "write a file".to_owned(),
                description: "Update README.md".to_owned(),
                display: Vec::new(),
                choices: vec![
                    choice("Approve once", ApprovalDecision::Approved),
                    choice(
                        "Approve for this session",
                        ApprovalDecision::ApprovedForSession,
                    ),
                    choice("Reject", ApprovalDecision::Rejected),
                    ApprovalPanelChoice {
                        label: "Reject with feedback".to_owned(),
                        response: ApprovalDecision::Rejected,
                        selected_label: None,
                        requires_feedback: true,
                        description: None,
                    },
                ],
            },
        }
    }

    type Responses = Arc<Mutex<Vec<ApprovalPanelResponse>>>;

    fn make_panel(request: PendingApproval) -> (ApprovalPanelComponent, Responses) {
        let responses = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&responses);
        let panel = ApprovalPanelComponent::new(request, move |response| {
            recorded.lock().expect("approval responses").push(response);
        });
        (panel, responses)
    }

    fn plain(text: &str) -> String {
        let ansi = regex::Regex::new("\\x1b\\[[0-9;]*m").expect("ANSI regex");
        ansi.replace_all(text, "").into_owned()
    }

    #[test]
    fn numeric_selection_feedback_editing_and_cancel_paths_match_source() {
        let (mut panel, responses) = make_panel(pending());
        panel.handle_input_event("2");
        assert_eq!(
            responses.lock().expect("approval responses")[0].response,
            ApprovalDecision::ApprovedForSession
        );

        let (mut panel, responses) = make_panel(pending());
        panel.handle_input_event("4");
        for input in ["n", "o", "\u{1b}[D", "!", "\r"] {
            panel.handle_input_event(input);
        }
        assert_eq!(
            responses.lock().expect("approval responses")[0]
                .feedback
                .as_deref(),
            Some("n!o")
        );

        let (mut panel, responses) = make_panel(pending());
        panel.handle_input_event("\u{1b}");
        assert_eq!(
            responses.lock().expect("approval responses")[0].response,
            ApprovalDecision::Rejected
        );
    }

    #[test]
    fn renders_inline_focused_feedback_descriptions_and_numeric_hint() {
        let mut request = pending();
        request.data.choices[0].description = Some("Approve this action only once.".to_owned());
        let (mut panel, _) = make_panel(request);
        panel.focused = true;
        panel.handle_input_event("4");
        let rendered = panel.render_panel(80);
        let stripped = plain(&rendered.join("\n"));
        assert!(stripped.contains("◆ 4. Reject with feedback"));
        assert!(!stripped.contains("\n  > "));
        assert!(
            rendered
                .join("")
                .contains(crate::tui::components::core::CURSOR_MARKER)
        );
        panel.handle_input_event("\u{1b}[A");
        let normal = plain(&panel.render_panel(80).join("\n"));
        assert!(normal.contains("Approve this action only once."));
        assert!(normal.contains("1/2/3/4 choose"));
    }

    #[test]
    fn renders_shell_wraps_full_command_and_danger_without_icon() {
        let mut request = pending();
        request.data.tool_name = "Bash".to_owned();
        request.data.description.clear();
        request.data.display = vec![DisplayBlock::Shell {
            language: "bash".to_owned(),
            command: format!(
                "printf approve-long-command-head_{}_approve-long-command-tail",
                "x".repeat(220)
            ),
            cwd: Some("/work".to_owned()),
            description: None,
            danger: Some("recursive delete".to_owned()),
        }];
        let (mut panel, _) = make_panel(request);
        let rendered = plain(&panel.render_panel(60).join("\n"));
        assert!(rendered.contains("Run this command?"));
        assert!(rendered.contains("Dangerous: recursive delete"));
        assert!(rendered.contains("approve-long-command-head"));
        assert!(rendered.contains("approve-long-command-tail"));
        assert!(!rendered.contains('⚠'));
    }

    #[test]
    fn compact_diff_and_file_content_delegate_first_previewable_block() {
        let diff = DiffDisplayBlock {
            path: "src/foo.rs".to_owned(),
            old_text: (1..=30)
                .map(|n| format!("old{n}"))
                .collect::<Vec<_>>()
                .join("\n"),
            new_text: (1..=30)
                .map(|n| format!("new{n}"))
                .collect::<Vec<_>>()
                .join("\n"),
            old_start: None,
            new_start: None,
            is_summary: None,
        };
        let mut request = pending();
        request.data.tool_name = "Edit".to_owned();
        request.data.description.clear();
        request.data.display = vec![DisplayBlock::Diff(diff.clone())];
        let previews = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&previews);
        let mut panel = ApprovalPanelComponent::new_with_callbacks(
            request,
            |_| {},
            None,
            Some(Box::new(move |block| {
                recorded.lock().expect("previews").push(block)
            })),
        );
        let compact = plain(&panel.render_panel(120).join("\n"));
        assert!(compact.contains("+30"));
        assert!(compact.contains("ctrl+e preview"));
        assert!(!compact.contains("new30"));
        panel.handle_input_event("\u{5}");
        assert_eq!(
            *previews.lock().expect("previews"),
            [ApprovalPreviewBlock::Diff(diff)]
        );

        let block = FileContentDisplayBlock {
            path: "src/new.rs".to_owned(),
            content: (1..=30)
                .map(|n| format!("const X{n}: usize = {n};"))
                .collect::<Vec<_>>()
                .join("\n"),
            language: None,
        };
        let mut request = pending();
        request.data.display = vec![DisplayBlock::FileContent(block)];
        let (mut panel, _) = make_panel(request);
        let compact = plain(&panel.render_panel(120).join("\n"));
        assert!(compact.contains("20 more lines hidden"));
        assert!(!compact.contains("X25"));
    }

    #[test]
    fn duplicate_brief_is_suppressed_and_display_is_capped_at_five_blocks() {
        let mut request = pending();
        request.data.description = "same description".to_owned();
        request.data.display = std::iter::once(DisplayBlock::Brief {
            text: "same description".to_owned(),
        })
        .chain((0..6).map(|index| DisplayBlock::Brief {
            text: format!("block {index}"),
        }))
        .collect();
        let (mut panel, _) = make_panel(request);
        let output = plain(&panel.render_panel(80).join("\n"));
        assert_eq!(output.matches("same description").count(), 0);
        assert!(output.contains("block 4"));
        assert!(!output.contains("block 5"));
    }

    #[test]
    fn helper_mappings_cover_headers_truncation_and_empty_hint() {
        assert_eq!(header_for("Write"), "Write this file?");
        assert_eq!(header_for("ExitPlanMode"), "Ready to build with this plan?");
        assert_eq!(header_for("Custom"), "Approve Custom?");
        assert_eq!(truncate_one_line("abcdef\nrest", 4), "abc…");
        assert_eq!(build_numeric_hint(0), "–");
        assert_eq!(build_numeric_hint(12), "1/2/3/4/5/6/7/8/9");
    }
}
