use std::{any::Any, collections::BTreeSet};

use crate::tui::{
    components::{
        Component, ComponentRole, Input, InputAction,
        render::{truncate_to_width, visible_width, wrap_text_with_ansi},
    },
    keys::{EditorKey, matches_editor_key},
    reverse_rpc::{PendingQuestion, QuestionPanelResponse, QuestionSubmissionMethod},
    theme::{ColorToken, current_theme},
    utils::printable_key::printable_char,
};

const NUMBER_KEYS: [&str; 9] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];
const MAX_BODY_LINES: usize = 12;
const DEFAULT_OTHER_LABEL: &str = "Other";
const NOT_ANSWERED_LABEL: &str = "Not answered";
const REVIEW_TITLE: &str = "Review your answer before submit";
const SUBMIT_PROMPT: &str = "Ready to submit your answers?";
const UNANSWERED_WARNING: &str = "Some questions are still unanswered.";
const SUBMIT_ACTIONS: [&str; 2] = ["Submit", "Cancel"];

type AnswerCallback = dyn FnMut(QuestionPanelResponse) + Send;
type ToggleCallback = dyn FnMut() + Send;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisplayOptionKind {
    Preset,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DisplayOption {
    label: String,
    description: Option<String>,
    kind: DisplayOptionKind,
}

/// Structured multi-question prompt with a final review tab.
///
/// Original: `question-dialog.ts`, `QuestionDialogComponent`.
pub struct QuestionDialogComponent {
    pub focused: bool,
    request: PendingQuestion,
    on_answer: Box<AnswerCallback>,
    max_visible_options: usize,
    other_input: Input,
    current_tab: usize,
    submit_action_index: usize,
    editing_other: bool,
    last_answer_method: Option<QuestionSubmissionMethod>,
    cursors: Vec<usize>,
    single_selections: Vec<Option<usize>>,
    multi_selections: Vec<BTreeSet<usize>>,
    other_drafts: Vec<String>,
    committed_other_values: Vec<Option<String>>,
    answers: Vec<Option<String>>,
    on_toggle_tool_output: Option<Box<ToggleCallback>>,
}

impl QuestionDialogComponent {
    pub fn new<F>(
        request: PendingQuestion,
        on_answer: F,
        max_visible_options: usize,
        on_toggle_tool_output: Option<Box<ToggleCallback>>,
    ) -> Self
    where
        F: FnMut(QuestionPanelResponse) + Send + 'static,
    {
        let total = request.data.questions.len();
        Self {
            focused: false,
            request,
            on_answer: Box::new(on_answer),
            max_visible_options,
            other_input: Input::new(),
            current_tab: 0,
            submit_action_index: 0,
            editing_other: false,
            last_answer_method: None,
            cursors: vec![0; total],
            single_selections: vec![None; total],
            multi_selections: vec![BTreeSet::new(); total],
            other_drafts: vec![String::new(); total],
            committed_other_values: vec![None; total],
            answers: vec![None; total],
            on_toggle_tool_output,
        }
    }

    pub fn with_defaults<F>(request: PendingQuestion, on_answer: F) -> Self
    where
        F: FnMut(QuestionPanelResponse) + Send + 'static,
    {
        Self::new(request, on_answer, 6, None)
    }

    /// Original: `QuestionDialogComponent.handleInput()`.
    pub fn handle_input_event(&mut self, data: &str) {
        if matches_editor_key(data, EditorKey::Escape)
            || matches_editor_key(data, EditorKey::Ctrl('c'))
            || matches_editor_key(data, EditorKey::Ctrl('d'))
        {
            (self.on_answer)(QuestionPanelResponse::cancelled());
            return;
        }
        if matches_editor_key(data, EditorKey::Ctrl('o')) {
            if let Some(callback) = &mut self.on_toggle_tool_output {
                callback();
            }
            return;
        }
        if self.is_editing_other() {
            self.handle_other_input(data);
            return;
        }
        if self.is_submit_tab() {
            self.handle_submit_input(data);
            return;
        }

        let Some(question_index) = self.current_question_index() else {
            return;
        };
        let option_count = self.display_options(question_index).len();
        if option_count == 0 {
            return;
        }
        if matches_editor_key(data, EditorKey::Up) {
            self.move_question_cursor(-1);
            return;
        }
        if matches_editor_key(data, EditorKey::Down) {
            self.move_question_cursor(1);
            return;
        }
        if matches_editor_key(data, EditorKey::Left) {
            self.goto_tab(self.current_tab as isize - 1);
            return;
        }
        if matches_editor_key(data, EditorKey::Right) || matches_editor_key(data, EditorKey::Tab) {
            self.goto_tab(self.current_tab as isize + 1);
            return;
        }
        if matches_editor_key(data, EditorKey::Enter) {
            self.activate_question_option(self.current_cursor(), QuestionSubmissionMethod::Enter);
            return;
        }

        let printable = printable_char(data);
        if let Some(number_index) = NUMBER_KEYS.iter().position(|number| *number == printable)
            && number_index < option_count
        {
            self.cursors[question_index] = number_index;
            self.activate_question_option(number_index, QuestionSubmissionMethod::NumberKey);
            return;
        }
        if printable == " " && self.request.data.questions[question_index].multi_select {
            self.activate_question_option(self.current_cursor(), QuestionSubmissionMethod::Space);
        }
    }

    /// Original: `QuestionDialogComponent.handleOtherInput()`.
    fn handle_other_input(&mut self, data: &str) {
        let Some(question_index) = self.current_question_index() else {
            return;
        };
        if matches_editor_key(data, EditorKey::Tab) {
            self.sync_other_draft(question_index);
            self.editing_other = false;
            self.goto_tab(self.current_tab as isize + 1);
            return;
        }
        if matches_editor_key(data, EditorKey::Up) {
            self.sync_other_draft(question_index);
            self.editing_other = false;
            self.move_question_cursor(-1);
            return;
        }
        if matches_editor_key(data, EditorKey::Down) {
            self.sync_other_draft(question_index);
            self.editing_other = false;
            self.move_question_cursor(1);
            return;
        }
        if let Some(InputAction::Submit(value)) = self.other_input.handle_input_event(data) {
            self.commit_other_input(Some(&value), QuestionSubmissionMethod::Enter);
        } else {
            self.sync_other_draft(question_index);
        }
    }

    /// Original: `QuestionDialogComponent.handleSubmitInput()`.
    fn handle_submit_input(&mut self, data: &str) {
        if matches_editor_key(data, EditorKey::Up) {
            self.submit_action_index =
                (self.submit_action_index + SUBMIT_ACTIONS.len() - 1) % SUBMIT_ACTIONS.len();
            return;
        }
        if matches_editor_key(data, EditorKey::Down) {
            self.submit_action_index = (self.submit_action_index + 1) % SUBMIT_ACTIONS.len();
            return;
        }
        if matches_editor_key(data, EditorKey::Left) {
            self.goto_tab(self.current_tab as isize - 1);
            return;
        }
        if matches_editor_key(data, EditorKey::Right) || matches_editor_key(data, EditorKey::Tab) {
            self.goto_tab(self.current_tab as isize + 1);
            return;
        }
        if matches_editor_key(data, EditorKey::Enter) {
            self.execute_submit_action(self.submit_action_index, QuestionSubmissionMethod::Enter);
            return;
        }
        match printable_char(data).as_str() {
            "1" => {
                self.submit_action_index = 0;
                self.execute_submit_action(0, QuestionSubmissionMethod::NumberKey);
            }
            "2" => {
                self.submit_action_index = 1;
                self.execute_submit_action(1, QuestionSubmissionMethod::NumberKey);
            }
            _ => {}
        }
    }

    /// Original: `QuestionDialogComponent.gotoTab()`.
    fn goto_tab(&mut self, target: isize) {
        let total = self.total_tabs();
        if total == 0 {
            return;
        }
        let wrapped = target.rem_euclid(total as isize) as usize;
        if wrapped == self.current_tab {
            return;
        }
        self.current_tab = wrapped;
        self.editing_other = false;
        if self.is_submit_tab() {
            self.submit_action_index = 0;
        }
    }

    /// Original: `QuestionDialogComponent.moveQuestionCursor()`.
    fn move_question_cursor(&mut self, delta: isize) {
        let Some(question_index) = self.current_question_index() else {
            return;
        };
        let total = self.display_options(question_index).len();
        if total == 0 {
            return;
        }
        self.cursors[question_index] =
            (self.current_cursor() as isize + delta).rem_euclid(total as isize) as usize;
    }

    /// Original: `QuestionDialogComponent.activateQuestionOption()`.
    fn activate_question_option(&mut self, option_index: usize, method: QuestionSubmissionMethod) {
        let Some(question_index) = self.current_question_index() else {
            return;
        };
        self.cursors[question_index] = option_index;
        self.editing_other = false;
        if self.is_other_option(question_index, option_index) {
            self.enter_other_input(question_index);
            return;
        }
        let multi_select = self.request.data.questions[question_index].multi_select;
        if multi_select {
            let selections = &mut self.multi_selections[question_index];
            if !selections.remove(&option_index) {
                selections.insert(option_index);
            }
            self.last_answer_method = Some(method);
            self.update_answer(question_index);
            return;
        }
        self.single_selections[question_index] = Some(option_index);
        self.committed_other_values[question_index] = None;
        self.last_answer_method = Some(method);
        self.update_answer(question_index);
        self.advance_after_single_select(question_index);
    }

    /// Original: `QuestionDialogComponent.enterOtherInput()`.
    fn enter_other_input(&mut self, question_index: usize) {
        self.cursors[question_index] = self.other_option_index(question_index);
        self.editing_other = true;
        let draft = self.other_draft_value(question_index).to_owned();
        self.other_input.set_value(draft);
    }

    /// Original: `QuestionDialogComponent.commitOtherInput()`.
    fn commit_other_input(&mut self, raw_value: Option<&str>, method: QuestionSubmissionMethod) {
        let Some(question_index) = self.current_question_index() else {
            return;
        };
        let value = raw_value
            .unwrap_or(self.other_input.value())
            .trim()
            .to_owned();
        if value.is_empty() {
            return;
        }
        self.other_input.set_value(&value);
        self.other_drafts[question_index] = value.clone();
        self.committed_other_values[question_index] = Some(value);
        let other_index = self.other_option_index(question_index);
        let multi_select = self.request.data.questions[question_index].multi_select;
        if multi_select {
            self.multi_selections[question_index].insert(other_index);
        } else {
            self.single_selections[question_index] = Some(other_index);
        }
        self.last_answer_method = Some(method);
        self.update_answer(question_index);
        self.editing_other = false;
        if !multi_select {
            self.advance_after_single_select(question_index);
        }
    }

    /// Original: `QuestionDialogComponent.advanceAfterSingleSelect()`.
    fn advance_after_single_select(&mut self, question_index: usize) {
        self.current_tab = self
            .find_next_unanswered_after(question_index)
            .unwrap_or_else(|| self.submit_tab_index());
        if self.is_submit_tab() {
            self.submit_action_index = 0;
        }
    }

    /// Original: `QuestionDialogComponent.findNextUnansweredAfter()`.
    fn find_next_unanswered_after(&self, from_index: usize) -> Option<usize> {
        (from_index + 1..self.request.data.questions.len()).find(|index| !self.is_answered(*index))
    }

    /// Original: `QuestionDialogComponent.updateAnswer()`.
    fn update_answer(&mut self, question_index: usize) {
        let question = &self.request.data.questions[question_index];
        if question.multi_select {
            let selections = &self.multi_selections[question_index];
            let mut labels = question
                .options
                .iter()
                .enumerate()
                .filter(|(index, option)| selections.contains(index) && !option.label.is_empty())
                .map(|(_, option)| option.label.clone())
                .collect::<Vec<_>>();
            let other_index = question.options.len();
            if selections.contains(&other_index)
                && let Some(other) = self.committed_other_values[question_index]
                    .as_deref()
                    .filter(|value| !value.is_empty())
            {
                labels.push(other.to_owned());
            }
            self.answers[question_index] = (!labels.is_empty()).then(|| labels.join(", "));
            return;
        }
        self.answers[question_index] =
            self.single_selections[question_index].and_then(|selection| {
                if selection == question.options.len() {
                    self.committed_other_values[question_index]
                        .as_deref()
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned)
                } else {
                    question
                        .options
                        .get(selection)
                        .map(|option| option.label.as_str())
                        .filter(|label| !label.is_empty())
                        .map(str::to_owned)
                }
            });
    }

    /// Original: `QuestionDialogComponent.executeSubmitAction()`.
    fn execute_submit_action(&mut self, action_index: usize, method: QuestionSubmissionMethod) {
        if action_index == 1 {
            (self.on_answer)(QuestionPanelResponse::cancelled());
        } else {
            self.emit_answers(method);
        }
    }

    /// Original: `QuestionDialogComponent.emitAnswers()`.
    fn emit_answers(&mut self, method: QuestionSubmissionMethod) {
        let last_answered = self.answers.iter().rposition(Option::is_some);
        let answers = last_answered.map_or_else(Vec::new, |last| self.answers[..=last].to_vec());
        (self.on_answer)(QuestionPanelResponse {
            answers,
            method: Some(self.last_answer_method.unwrap_or(method)),
        });
    }

    /// Original: `QuestionDialogComponent.render()`.
    pub fn render_dialog(&mut self, width: usize) -> Vec<String> {
        self.other_input.focused = self.focused && self.is_editing_other();
        if self.is_submit_tab() {
            self.render_submit_tab(width)
        } else {
            self.render_question_tab(width)
        }
    }

    /// Original: `QuestionDialogComponent.renderQuestionTab()`.
    fn render_question_tab(&self, width: usize) -> Vec<String> {
        let Some(question_index) = self.current_question_index() else {
            return self.render_submit_tab(width);
        };
        let Some(question) = self.request.data.questions.get(question_index) else {
            return Vec::new();
        };
        let render_width = width.max(1);
        let theme = current_theme();
        let mut lines = vec![
            theme.fg(ColorToken::Primary, &"─".repeat(render_width)),
            theme.bold_fg(ColorToken::Primary, " question"),
            String::new(),
        ];
        self.push_tabs(&mut lines);
        lines.push(String::new());
        append_wrapped(
            &mut lines,
            " ? ",
            "   ",
            &question.question,
            render_width,
            Some(ColorToken::Primary),
            false,
        );
        if self.is_editing_other() {
            lines.push(theme.fg(
                ColorToken::TextDim,
                "   Type your answer, then press Enter to save.",
            ));
        }
        if let Some(body) = question
            .body
            .as_deref()
            .filter(|body| !body.trim().is_empty())
        {
            lines.push(String::new());
            let body_lines = body.trim().lines().collect::<Vec<_>>();
            for body_line in body_lines.iter().take(MAX_BODY_LINES) {
                append_wrapped(
                    &mut lines,
                    "   ",
                    "   ",
                    body_line,
                    render_width,
                    Some(ColorToken::TextDim),
                    false,
                );
            }
            if body_lines.len() > MAX_BODY_LINES {
                lines.push(theme.fg(
                    ColorToken::TextDim,
                    &format!("   ... {} more lines", body_lines.len() - MAX_BODY_LINES),
                ));
            }
        }
        lines.push(String::new());

        let options = self.display_options(question_index);
        let cursor = self.current_cursor();
        let visible_start = self.compute_visible_start(cursor, options.len());
        let visible_end = options
            .len()
            .min(visible_start.saturating_add(self.max_visible_options));
        for (index, option) in options
            .iter()
            .enumerate()
            .take(visible_end)
            .skip(visible_start)
        {
            let number = index + 1;
            let is_cursor = index == cursor;
            let is_other = option.kind == DisplayOptionKind::Other;
            let is_selected = if question.multi_select {
                self.multi_selections[question_index].contains(&index)
            } else {
                self.single_selections[question_index] == Some(index)
            };
            if self.is_editing_other() && is_cursor && is_other {
                lines.push(self.render_editing_other_line(
                    render_width,
                    question_index,
                    option,
                    number,
                    is_selected,
                ));
                continue;
            }
            let label = self.render_option_label(question_index, option, is_cursor);
            let (prefix, tone, bold) = if question.multi_select {
                let checked = if is_selected { "✓" } else { " " };
                (
                    format!("  [{checked}] "),
                    if is_selected {
                        ColorToken::Success
                    } else if is_cursor {
                        ColorToken::Primary
                    } else {
                        ColorToken::TextDim
                    },
                    is_selected && is_cursor,
                )
            } else if is_selected && self.is_answered(question_index) {
                (
                    if is_cursor {
                        format!("  ❯[{number}] ")
                    } else {
                        format!("    [{number}] ")
                    },
                    ColorToken::Success,
                    is_cursor,
                )
            } else if is_cursor {
                (format!("  ❯[{number}] "), ColorToken::Primary, false)
            } else {
                (format!("    [{number}] "), ColorToken::TextDim, false)
            };
            let continuation = " ".repeat(visible_width(&prefix));
            append_wrapped(
                &mut lines,
                &prefix,
                &continuation,
                &label,
                render_width,
                Some(tone),
                bold,
            );
            if let Some(description) = option
                .description
                .as_deref()
                .filter(|text| !text.is_empty())
            {
                append_wrapped(
                    &mut lines,
                    "        ",
                    "        ",
                    description,
                    render_width,
                    Some(ColorToken::TextDim),
                    false,
                );
            }
        }
        if visible_end < options.len() || visible_start > 0 {
            lines.push(theme.fg(
                ColorToken::TextDim,
                &format!(
                    "   showing {}-{visible_end} of {}",
                    visible_start + 1,
                    options.len()
                ),
            ));
        }
        lines.push(String::new());
        lines.push(self.build_question_hint(question_index));
        lines.push(theme.fg(ColorToken::Primary, &"─".repeat(render_width)));
        bound_lines(lines, width)
    }

    /// Original: `QuestionDialogComponent.renderSubmitTab()`.
    fn render_submit_tab(&self, width: usize) -> Vec<String> {
        let render_width = width.max(1);
        let theme = current_theme();
        let mut lines = vec![
            theme.fg(ColorToken::Primary, &"─".repeat(render_width)),
            theme.bold_fg(ColorToken::Primary, " question"),
            String::new(),
        ];
        self.push_tabs(&mut lines);
        lines.push(String::new());
        lines.push(theme.bold_fg(ColorToken::Text, &format!(" {REVIEW_TITLE}")));
        if self.has_unanswered_questions() {
            lines.push(theme.fg(ColorToken::Warning, &format!("  {UNANSWERED_WARNING}")));
        }
        lines.push(String::new());
        for (index, question) in self.request.data.questions.iter().enumerate() {
            append_wrapped(
                &mut lines,
                &format!("  {}  ", theme.fg(ColorToken::TextDim, "Q")),
                "       ",
                &question.question,
                render_width,
                None,
                false,
            );
            if let Some(answer) = self.answers[index]
                .as_deref()
                .filter(|answer| !answer.is_empty())
            {
                append_wrapped(
                    &mut lines,
                    &format!("  {}  ", theme.fg(ColorToken::Primary, "❯")),
                    "       ",
                    &theme.fg(ColorToken::Text, answer),
                    render_width,
                    None,
                    false,
                );
            } else {
                lines.push(format!(
                    "  {}  {}",
                    theme.fg(ColorToken::TextDim, "❯"),
                    theme.fg(ColorToken::TextDim, NOT_ANSWERED_LABEL)
                ));
            }
        }
        lines.extend([
            String::new(),
            theme.fg(ColorToken::Text, &format!(" {SUBMIT_PROMPT}")),
            String::new(),
        ]);
        for (index, label) in SUBMIT_ACTIONS.iter().enumerate() {
            let text = if index == self.submit_action_index {
                format!("  ❯[{}] {label}", index + 1)
            } else {
                format!("    [{}] {label}", index + 1)
            };
            lines.push(theme.fg(
                if index == self.submit_action_index {
                    ColorToken::Primary
                } else {
                    ColorToken::TextDim
                },
                &text,
            ));
        }
        lines.push(String::new());
        lines.push(self.build_submit_hint());
        lines.push(theme.fg(ColorToken::Primary, &"─".repeat(render_width)));
        bound_lines(lines, width)
    }

    /// Original: `QuestionDialogComponent.pushTabs()`.
    fn push_tabs(&self, lines: &mut Vec<String>) {
        let theme = current_theme();
        let mut tabs = Vec::new();
        for (index, question) in self.request.data.questions.iter().enumerate() {
            let label = question
                .header
                .as_deref()
                .filter(|label| !label.is_empty())
                .map_or_else(|| format!("Q{}", index + 1), str::to_owned);
            if index == self.current_tab {
                tabs.push(theme.bg(
                    ColorToken::Primary,
                    &theme.bold_fg(ColorToken::Text, &format!(" {label} ")),
                ));
            } else if self.is_answered(index) {
                tabs.push(theme.fg(ColorToken::Success, &format!("(✓) {label}")));
            } else {
                tabs.push(theme.fg(ColorToken::TextDim, &format!("(○) {label}")));
            }
        }
        if self.is_submit_tab() {
            tabs.push(theme.bg(
                ColorToken::Primary,
                &theme.bold_fg(ColorToken::Text, " Submit "),
            ));
        } else {
            tabs.push(theme.fg(ColorToken::TextDim, " Submit "));
        }
        lines.push(format!(" {}", tabs.join("  ")));
    }

    fn build_question_hint(&self, question_index: usize) -> String {
        let theme = current_theme();
        if self.is_editing_other() {
            let mut parts = vec!["type answer", "↵ save"];
            if self.total_tabs() > 1 {
                parts.push("tab switch");
            }
            parts.push("esc cancel");
            return theme.fg(ColorToken::TextDim, &format!("  {}", parts.join("  ")));
        }
        let option_count = self
            .display_options(question_index)
            .len()
            .min(NUMBER_KEYS.len());
        let number_hint = if option_count <= 1 {
            "1".to_owned()
        } else {
            format!("1-{option_count}")
        };
        let question = &self.request.data.questions[question_index];
        let mut parts = vec![
            "↑↓ select".to_owned(),
            format!(
                "{number_hint} / ↵ {}",
                if question.multi_select {
                    "toggle"
                } else {
                    "choose"
                }
            ),
        ];
        if self.total_tabs() > 1 {
            parts.push("←→ tab switch".to_owned());
        }
        parts.push("esc cancel".to_owned());
        theme.fg(ColorToken::TextDim, &format!("  {}", parts.join("  ")))
    }

    fn build_submit_hint(&self) -> String {
        let mut parts = vec!["↑↓ select", "1/2 choose", "↵ confirm"];
        if self.total_tabs() > 1 {
            parts.push("←→ tab switch");
        }
        parts.push("esc cancel");
        current_theme().fg(ColorToken::TextDim, &format!("  {}", parts.join("  ")))
    }

    /// Original: `QuestionDialogComponent.computeVisibleStart()`.
    fn compute_visible_start(&self, cursor: usize, total: usize) -> usize {
        if total <= self.max_visible_options {
            return 0;
        }
        let half = self.max_visible_options / 2;
        cursor
            .saturating_sub(half)
            .min(total.saturating_sub(self.max_visible_options))
    }

    fn total_tabs(&self) -> usize {
        self.request.data.questions.len() + 1
    }

    fn submit_tab_index(&self) -> usize {
        self.request.data.questions.len()
    }

    fn is_submit_tab(&self) -> bool {
        self.current_tab == self.submit_tab_index()
    }

    fn is_editing_other(&self) -> bool {
        self.editing_other && !self.is_submit_tab()
    }

    fn current_question_index(&self) -> Option<usize> {
        (!self.is_submit_tab()).then_some(self.current_tab)
    }

    fn current_cursor(&self) -> usize {
        self.current_question_index()
            .and_then(|index| self.cursors.get(index).copied())
            .unwrap_or(0)
    }

    /// Original: `QuestionDialogComponent.displayOptions()`.
    fn display_options(&self, question_index: usize) -> Vec<DisplayOption> {
        let Some(question) = self.request.data.questions.get(question_index) else {
            return Vec::new();
        };
        let mut options = question
            .options
            .iter()
            .map(|option| DisplayOption {
                label: option.label.clone(),
                description: option.description.clone(),
                kind: DisplayOptionKind::Preset,
            })
            .collect::<Vec<_>>();
        options.push(DisplayOption {
            label: question
                .other_label
                .as_deref()
                .filter(|label| !label.is_empty())
                .unwrap_or(DEFAULT_OTHER_LABEL)
                .to_owned(),
            description: question
                .other_description
                .as_deref()
                .filter(|description| !description.is_empty())
                .map(str::to_owned),
            kind: DisplayOptionKind::Other,
        });
        options
    }

    fn other_option_index(&self, question_index: usize) -> usize {
        self.request
            .data
            .questions
            .get(question_index)
            .map_or(0, |question| question.options.len())
    }

    fn is_other_option(&self, question_index: usize, option_index: usize) -> bool {
        option_index == self.other_option_index(question_index)
    }

    fn render_option_label(
        &self,
        question_index: usize,
        option: &DisplayOption,
        _is_cursor: bool,
    ) -> String {
        if option.kind != DisplayOptionKind::Other {
            return option.label.clone();
        }
        let value = self.other_draft_value(question_index);
        if value.is_empty() {
            option.label.clone()
        } else {
            format!("{}: {value}", option.label)
        }
    }

    fn render_editing_other_line(
        &self,
        width: usize,
        question_index: usize,
        option: &DisplayOption,
        number: usize,
        is_selected: bool,
    ) -> String {
        let question = &self.request.data.questions[question_index];
        let body = if question.multi_select {
            format!(
                "  [{}] {}: ",
                if is_selected { "✓" } else { " " },
                option.label
            )
        } else {
            format!("  ❯[{number}] {}: ", option.label)
        };
        let prefix = if is_selected && self.is_answered(question_index) {
            current_theme().bold_fg(ColorToken::Success, &body)
        } else {
            current_theme().fg(ColorToken::Primary, &body)
        };
        let input_width = width
            .saturating_sub(visible_width(&prefix))
            .saturating_add(2)
            .max(4);
        let input_line = self.other_input.render_line(input_width);
        format!(
            "{prefix}{}",
            input_line.strip_prefix("> ").unwrap_or(&input_line)
        )
    }

    fn other_draft_value(&self, question_index: usize) -> &str {
        self.other_drafts
            .get(question_index)
            .map(String::as_str)
            .unwrap_or("")
    }

    fn sync_other_draft(&mut self, question_index: usize) {
        self.other_drafts[question_index] = self.other_input.value().to_owned();
    }

    fn is_answered(&self, question_index: usize) -> bool {
        self.answers
            .get(question_index)
            .and_then(Option::as_deref)
            .is_some_and(|answer| !answer.is_empty())
    }

    fn has_unanswered_questions(&self) -> bool {
        (0..self.request.data.questions.len()).any(|index| !self.is_answered(index))
    }
}

impl Component for QuestionDialogComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_dialog(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.handle_input_event(data);
    }

    fn wants_key_release(&self) -> bool {
        true
    }

    fn invalidate(&mut self) {
        self.other_input.invalidate();
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn append_wrapped(
    lines: &mut Vec<String>,
    first_prefix: &str,
    continuation_prefix: &str,
    content: &str,
    width: usize,
    tone: Option<ColorToken>,
    bold: bool,
) {
    let prefix_width = visible_width(first_prefix).max(visible_width(continuation_prefix));
    let content_width = width.saturating_sub(prefix_width).max(1);
    let wrapped = wrap_text_with_ansi(content, content_width);
    let style = |line: String| match (tone, bold) {
        (Some(token), true) => current_theme().bold_fg(token, &line),
        (Some(token), false) => current_theme().fg(token, &line),
        (None, _) => line,
    };
    if wrapped.is_empty() {
        lines.push(style(first_prefix.to_owned()));
        return;
    }
    lines.push(style(format!("{first_prefix}{}", wrapped[0])));
    for line in wrapped.iter().skip(1) {
        lines.push(style(format!("{continuation_prefix}{line}")));
    }
}

fn bound_lines(lines: Vec<String>, width: usize) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| truncate_to_width(&line, width, "...", false))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::tui::reverse_rpc::{QuestionPanelData, QuestionPanelItem, QuestionPanelOption};

    use super::*;

    fn item(question: &str, multi_select: bool, labels: &[&str]) -> QuestionPanelItem {
        QuestionPanelItem {
            question: question.to_owned(),
            header: None,
            body: None,
            multi_select,
            other_label: None,
            other_description: None,
            options: labels
                .iter()
                .map(|label| QuestionPanelOption {
                    label: (*label).to_owned(),
                    description: None,
                })
                .collect(),
        }
    }

    fn pending(questions: Vec<QuestionPanelItem>) -> PendingQuestion {
        PendingQuestion {
            data: QuestionPanelData {
                id: "q_1".to_owned(),
                tool_call_id: "tc_1".to_owned(),
                questions,
            },
        }
    }

    type Responses = Arc<Mutex<Vec<QuestionPanelResponse>>>;

    fn dialog(questions: Vec<QuestionPanelItem>) -> (QuestionDialogComponent, Responses) {
        let responses = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&responses);
        let dialog = QuestionDialogComponent::with_defaults(pending(questions), move |response| {
            recorded.lock().expect("question responses").push(response);
        });
        (dialog, responses)
    }

    fn plain(lines: &[String]) -> String {
        let ansi = regex::Regex::new("\\x1b\\[[0-9;]*m").expect("ANSI regex");
        ansi.replace_all(&lines.join("\n"), "").into_owned()
    }

    #[test]
    fn single_select_auto_advances_and_submits_only_from_review() {
        let (mut dialog, responses) = dialog(vec![
            item("Q1?", false, &["A1", "B1"]),
            item("Q2?", false, &["A2", "B2"]),
        ]);
        dialog.handle_input_event("2");
        assert!(plain(&dialog.render_dialog(80)).contains("Q2?"));
        dialog.handle_input_event("\r");
        let review = plain(&dialog.render_dialog(80));
        assert!(review.contains(REVIEW_TITLE));
        assert!(review.contains("B1"));
        assert!(review.contains("A2"));
        assert!(responses.lock().expect("question responses").is_empty());
        dialog.handle_input_event("1");
        assert_eq!(
            *responses.lock().expect("question responses"),
            [QuestionPanelResponse {
                answers: vec![Some("B1".to_owned()), Some("A2".to_owned())],
                method: Some(QuestionSubmissionMethod::Enter)
            }]
        );
    }

    #[test]
    fn unanswered_submit_preserves_sparse_position_and_cancel_is_empty() {
        let (mut first_dialog, responses) = dialog(vec![
            item("Q1?", false, &["A1"]),
            item("Q2?", false, &["A2", "B2"]),
        ]);
        first_dialog.handle_input_event("\t");
        first_dialog.handle_input_event("2");
        assert!(plain(&first_dialog.render_dialog(80)).contains(UNANSWERED_WARNING));
        first_dialog.handle_input_event("\r");
        assert_eq!(
            responses.lock().expect("question responses")[0].answers,
            [None, Some("B2".to_owned())]
        );

        let (mut dialog, responses) = dialog(vec![item("Q?", false, &["A"])]);
        dialog.handle_input_event("\t");
        dialog.handle_input_event("2");
        assert_eq!(
            *responses.lock().expect("question responses"),
            [QuestionPanelResponse::cancelled()]
        );
    }

    #[test]
    fn multi_select_toggles_in_option_order() {
        let (mut dialog, _) = dialog(vec![item("Pick many?", true, &["A", "B", "C"])]);
        dialog.handle_input_event(" ");
        dialog.handle_input_event("\u{1b}[B");
        dialog.handle_input_event("\u{1b}[B");
        dialog.handle_input_event("3");
        dialog.handle_input_event("\t");
        assert!(plain(&dialog.render_dialog(80)).contains("A, C"));
    }

    #[test]
    fn other_draft_survives_tabs_and_commits_inline_with_cursor_editing() {
        let mut question = item("Pick one?", false, &["A", "B"]);
        question.other_label = Some("Custom".to_owned());
        question.other_description = Some("Type your own answer".to_owned());
        let (mut dialog, responses) = dialog(vec![question]);
        dialog.focused = true;
        dialog.handle_input_event("3");
        assert!(
            dialog
                .render_dialog(80)
                .join("")
                .contains(crate::tui::components::core::CURSOR_MARKER)
        );
        for input in ["H", "i", "\u{1b}[D", "!", "\r"] {
            dialog.handle_input_event(input);
        }
        assert!(plain(&dialog.render_dialog(80)).contains("H!i"));
        dialog.handle_input_event("1");
        assert_eq!(
            responses.lock().expect("question responses")[0].answers,
            [Some("H!i".to_owned())]
        );
    }

    #[test]
    fn escape_controls_and_tool_toggle_route_without_mutating_answers() {
        let toggles = Arc::new(Mutex::new(0));
        let responses = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&responses);
        let toggled = Arc::clone(&toggles);
        let mut dialog = QuestionDialogComponent::new(
            pending(vec![item("Q?", false, &["A"])]),
            move |response| recorded.lock().expect("question responses").push(response),
            6,
            Some(Box::new(move || {
                *toggled.lock().expect("toggle count") += 1
            })),
        );
        dialog.handle_input_event("\u{f}");
        assert_eq!(*toggles.lock().expect("toggle count"), 1);
        assert!(responses.lock().expect("question responses").is_empty());
        dialog.handle_input_event("\u{3}");
        assert_eq!(
            *responses.lock().expect("question responses"),
            [QuestionPanelResponse::cancelled()]
        );
    }

    #[test]
    fn wraps_long_content_caps_body_and_keeps_every_line_bounded() {
        let mut question = item(
            "Please confirm whether this dangerous shell command should really be executed in the current workspace including every side effect.",
            false,
            &[
                "Apply changes to every file under the current workspace including nested submodules",
            ],
        );
        question.body = Some(
            (0..14)
                .map(|index| format!("body line {index}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        question.options[0].description = Some(
            "This option has a long description which must wrap without losing any words."
                .to_owned(),
        );
        let (mut dialog, _) = dialog(vec![question]);
        let lines = dialog.render_dialog(40);
        let rendered = plain(&lines);
        assert!(rendered.contains("... 2 more lines"));
        assert!(rendered.contains("nested submodules"));
        assert!(lines.iter().all(|line| visible_width(line) <= 40));
        assert_eq!(visible_width(&lines[0]), 40);
        assert_eq!(visible_width(lines.last().expect("bottom border")), 40);
    }
}
