use std::any::Any;

use crate::tui::{
    components::{
        Component, ComponentRole, Markdown, MarkdownOptions, Text,
        render::{truncate_to_width, visible_width},
    },
    theme::{ColorToken, current_theme},
};

const THINKING_PREVIEW_LINES: usize = 2;
const MIN_COLLAPSED_PANEL_LINES: usize = 3;
const ELLIPSIS: &str = "…";

type CanUseScrollKeys = dyn Fn() -> bool + Send;
type PromptHandler = dyn FnMut(String) + Send;
type TerminalRows = dyn Fn() -> usize + Send;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BtwPanelPhase {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BtwTurn {
    prompt: String,
    answer: String,
    thinking: String,
    error: Option<String>,
    phase: BtwPanelPhase,
}

struct BtwBodyRender {
    lines: Vec<String>,
    truncated: bool,
}

pub struct BtwPanelOptions {
    pub markdown_options: MarkdownOptions,
    pub can_use_scroll_keys: Box<CanUseScrollKeys>,
    pub on_prompt: Box<PromptHandler>,
    /// Returns zero when the terminal height is not known.
    pub terminal_rows: Box<TerminalRows>,
}

/// Stateful side-question panel rendered immediately above the editor.
///
/// Original: `src/tui/components/panes/btw-panel.ts`, `BtwPanelComponent`.
/// The TypeScript `MarkdownTheme` argument maps to the existing Rust
/// `MarkdownOptions`; terminal colors continue to come from `current_theme()`.
pub struct BtwPanelComponent {
    options: BtwPanelOptions,
    turns: Vec<BtwTurn>,
    transient_notices: Vec<String>,
    min_body_lines: usize,
    follow_tail: bool,
    scroll_top: usize,
    max_scroll_top: usize,
}

impl BtwPanelComponent {
    pub fn new(options: BtwPanelOptions) -> Self {
        Self {
            options,
            turns: Vec::new(),
            transient_notices: Vec::new(),
            min_body_lines: 0,
            follow_tail: true,
            scroll_top: 0,
            max_scroll_top: 0,
        }
    }

    // Original: BtwPanelComponent.submit()
    pub fn submit(&mut self, prompt: &str) {
        let normalized = prompt.trim();
        if normalized.is_empty() || self.is_running() {
            return;
        }
        self.follow_tail = true;
        self.scroll_top = 0;
        self.transient_notices.clear();
        self.turns.push(BtwTurn {
            prompt: normalized.to_owned(),
            answer: String::new(),
            thinking: String::new(),
            error: None,
            phase: BtwPanelPhase::Running,
        });
        (self.options.on_prompt)(normalized.to_owned());
    }

    pub fn add_transient_notice(&mut self, message: impl Into<String>) {
        self.transient_notices.push(message.into());
        self.follow_tail = true;
    }

    pub fn append_answer(&mut self, delta: &str) {
        if let Some(turn) = self.current_turn_mut() {
            turn.answer.push_str(delta);
        }
    }

    pub fn append_thinking(&mut self, delta: &str) {
        if let Some(turn) = self.current_turn_mut() {
            turn.thinking.push_str(delta);
        }
    }

    pub fn mark_done(&mut self, result_summary: Option<&str>) {
        let Some(turn) = self.current_turn_mut() else {
            return;
        };
        if turn.answer.trim().is_empty()
            && let Some(result_summary) = result_summary
        {
            turn.answer = result_summary.to_owned();
        }
        turn.phase = BtwPanelPhase::Done;
        self.transient_notices.clear();
    }

    pub fn mark_failed(&mut self, error: impl Into<String>) {
        let error = error.into();
        if let Some(turn) = self.current_turn_mut()
            && turn.phase == BtwPanelPhase::Running
        {
            turn.error = Some(error);
            turn.phase = BtwPanelPhase::Failed;
        } else {
            self.turns.push(BtwTurn {
                prompt: String::new(),
                answer: String::new(),
                thinking: String::new(),
                error: Some(error),
                phase: BtwPanelPhase::Failed,
            });
        }
        self.transient_notices.clear();
    }

    pub fn is_running(&self) -> bool {
        self.current_turn()
            .is_some_and(|turn| turn.phase == BtwPanelPhase::Running)
    }

    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    pub fn scroll(&mut self, direction: BtwScrollDirection) -> bool {
        if self.max_scroll_top == 0 {
            return false;
        }
        let current = if self.follow_tail {
            self.max_scroll_top
        } else {
            self.scroll_top
        };
        let next = match direction {
            BtwScrollDirection::Up => current.saturating_sub(1),
            BtwScrollDirection::Down => current.saturating_add(1).min(self.max_scroll_top),
        };
        self.scroll_top = next;
        self.follow_tail = next == self.max_scroll_top;
        true
    }

    fn render_top_border(&self, width: usize, truncated: bool) -> String {
        let theme = current_theme();
        let hint = if truncated && (self.options.can_use_scroll_keys)() {
            "Esc close · ↑↓ scroll "
        } else {
            "Esc close "
        };
        let title = format!(
            "{}{}{}",
            theme.bold_fg(ColorToken::Accent, " BTW "),
            theme.fg(ColorToken::Border, "─ "),
            theme.fg(ColorToken::TextMuted, hint)
        );
        let inner_width = width.saturating_sub(2).max(1);
        let clipped_title = if visible_width(&title) > inner_width {
            truncate_to_width(&title, inner_width, "", false)
        } else {
            title
        };
        let dash_count = inner_width.saturating_sub(visible_width(&clipped_title));
        format!(
            "{}{}{}{}",
            theme.fg(ColorToken::Border, "╭"),
            clipped_title,
            theme.fg(ColorToken::Border, &"─".repeat(dash_count)),
            theme.fg(ColorToken::Border, "╮")
        )
    }

    fn render_body(&mut self, width: usize) -> BtwBodyRender {
        let mut lines = Vec::new();
        for (index, turn) in self.turns.iter().enumerate() {
            if index > 0 {
                lines.push(String::new());
            }
            lines.extend(self.render_turn(turn, width));
        }
        if self.turns.is_empty() {
            lines.push(current_theme().fg(ColorToken::TextDim, "Ready for a side question..."));
        }
        lines.extend(self.render_transient_notices(width));
        self.fit_body_lines(lines)
    }

    fn render_transient_notices(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for notice in &self.transient_notices {
            let text = current_theme().fg(ColorToken::TextDim, notice);
            lines.extend(Text::new(text, 0, 0).render(width));
        }
        lines
    }

    fn fit_body_lines(&mut self, lines: Vec<String>) -> BtwBodyRender {
        let body_limit = self.collapsed_body_limit();
        let target_uncapped = self.min_body_lines.max(lines.len());
        let target = body_limit.map_or(target_uncapped, |limit| limit.min(target_uncapped));
        self.min_body_lines = self.min_body_lines.max(target);

        if lines.len() > target {
            self.max_scroll_top = lines.len() - target;
            if self.follow_tail {
                self.scroll_top = self.max_scroll_top;
            } else {
                self.scroll_top = self.scroll_top.min(self.max_scroll_top);
            }
            let start = self.scroll_top;
            return BtwBodyRender {
                lines: lines[start..start + target].to_vec(),
                truncated: true,
            };
        }

        self.follow_tail = true;
        self.scroll_top = 0;
        self.max_scroll_top = 0;
        let mut padded = lines;
        padded.resize(target, String::new());
        BtwBodyRender {
            lines: padded,
            truncated: false,
        }
    }

    fn collapsed_body_limit(&self) -> Option<usize> {
        let terminal_rows = (self.options.terminal_rows)();
        if terminal_rows == 0 {
            return None;
        }
        let max_panel_lines = MIN_COLLAPSED_PANEL_LINES.max(terminal_rows / 3);
        Some(max_panel_lines.saturating_sub(1).max(1))
    }

    fn render_turn(&self, turn: &BtwTurn, width: usize) -> Vec<String> {
        let prompt = current_theme().fg(ColorToken::Accent, &format!("Q: {}", turn.prompt));
        let mut lines = Text::new(prompt, 0, 0).render(width);
        let answer = turn.answer.trim();
        let thinking = turn.thinking.trim();
        if !answer.is_empty() {
            lines.extend(Markdown::new(answer, 0, 0, self.options.markdown_options).render(width));
        } else if !thinking.is_empty() {
            let thinking = current_theme().fg(ColorToken::TextDim, thinking);
            let thinking_lines = Text::new(thinking, 0, 0).render(width);
            let start = thinking_lines.len().saturating_sub(THINKING_PREVIEW_LINES);
            lines.extend(thinking_lines[start..].iter().cloned());
        } else if turn.error.is_none() {
            lines.push(current_theme().fg(ColorToken::TextDim, "Waiting for answer..."));
        }
        if let Some(error) = &turn.error {
            let error = current_theme().fg(ColorToken::Error, error);
            lines.extend(Text::new(error, 0, 0).render(width));
        }
        lines
    }

    fn render_body_line(&self, line: &str, width: usize) -> String {
        let theme = current_theme();
        let content_width = width.saturating_sub(4).max(1);
        let clipped = if visible_width(line) > content_width {
            truncate_to_width(line, content_width, ELLIPSIS, false)
        } else {
            line.to_owned()
        };
        let padding = content_width.saturating_sub(visible_width(&clipped));
        format!(
            "{} {clipped}{} {}",
            theme.fg(ColorToken::Border, "│"),
            " ".repeat(padding),
            theme.fg(ColorToken::Border, "│")
        )
    }

    fn current_turn(&self) -> Option<&BtwTurn> {
        self.turns.last()
    }

    fn current_turn_mut(&mut self) -> Option<&mut BtwTurn> {
        self.turns.last_mut()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtwScrollDirection {
    Up,
    Down,
}

impl Component for BtwPanelComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let safe_width = width.max(4);
        let content_width = safe_width.saturating_sub(4).max(1);
        let body = self.render_body(content_width);
        let mut lines = vec![self.render_top_border(safe_width, body.truncated)];
        lines.extend(
            body.lines
                .iter()
                .map(|line| self.render_body_line(line, safe_width)),
        );
        lines
    }

    fn invalidate(&mut self) {}

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use super::*;

    fn plain(text: &str) -> String {
        let mut result = String::new();
        let mut index = 0;
        while index < text.len() {
            if text.as_bytes()[index] == 0x1b {
                index += 1;
                if text.as_bytes().get(index) == Some(&b'[') {
                    index += 1;
                    while index < text.len() && text.as_bytes()[index] != b'm' {
                        index += 1;
                    }
                    index = (index + 1).min(text.len());
                    continue;
                }
            }
            let Some(character) = text[index..].chars().next() else {
                break;
            };
            result.push(character);
            index += character.len_utf8();
        }
        result
    }

    fn panel(
        prompts: Arc<Mutex<Vec<String>>>,
        rows: Arc<AtomicUsize>,
        scroll_keys: Arc<AtomicBool>,
    ) -> BtwPanelComponent {
        BtwPanelComponent::new(BtwPanelOptions {
            markdown_options: MarkdownOptions::default(),
            can_use_scroll_keys: Box::new(move || scroll_keys.load(Ordering::Relaxed)),
            on_prompt: Box::new(move |prompt| match prompts.lock() {
                Ok(mut prompts) => prompts.push(prompt),
                Err(poisoned) => poisoned.into_inner().push(prompt),
            }),
            terminal_rows: Box::new(move || rows.load(Ordering::Relaxed)),
        })
    }

    fn fixture() -> (BtwPanelComponent, Arc<Mutex<Vec<String>>>, Arc<AtomicUsize>) {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let rows = Arc::new(AtomicUsize::new(0));
        let component = panel(
            Arc::clone(&prompts),
            Arc::clone(&rows),
            Arc::new(AtomicBool::new(true)),
        );
        (component, prompts, rows)
    }

    #[test]
    fn starts_empty_and_submit_normalizes_and_dispatches_once() {
        let (mut panel, prompts, _) = fixture();
        assert!(panel.is_empty());
        assert!(plain(&panel.render(80).join("\n")).contains("Ready for a side question..."));

        panel.submit("  side question  ");
        panel.submit("ignored while running");
        assert!(panel.is_running());
        assert!(!panel.is_empty());
        let captured = match prompts.lock() {
            Ok(prompts) => prompts.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        assert_eq!(captured, ["side question"]);
        let rendered = plain(&panel.render(80).join("\n"));
        assert!(rendered.contains("Q: side question"));
        assert!(rendered.contains("Waiting for answer..."));
    }

    #[test]
    fn shows_only_thinking_tail_then_keeps_height_for_short_answer() {
        let (mut panel, _, _) = fixture();
        panel.submit("question");
        panel.append_thinking("line1\nline2\nline3\nline4\nline5\nline6\nline7");
        let thinking = plain(&panel.render(80).join("\n"));
        assert!(!thinking.contains("line5"));
        assert!(thinking.contains("line6"));
        assert!(thinking.contains("line7"));
        let thinking_height = panel.render(80).len();

        panel.append_answer("**final** answer");
        panel.mark_done(None);
        let answer_lines = panel.render(80);
        assert_eq!(answer_lines.len(), thinking_height);
        assert!(plain(&answer_lines.join("\n")).contains("final answer"));
        assert!(
            plain(answer_lines.last().map_or("", String::as_str))
                .trim_matches('│')
                .trim()
                .is_empty()
        );
    }

    #[test]
    fn result_summary_failure_and_notices_follow_original_transitions() {
        let (mut panel, _, _) = fixture();
        panel.add_transient_notice("starting side agent");
        assert!(plain(&panel.render(80).join("\n")).contains("starting side agent"));
        panel.submit("question");
        panel.mark_done(Some("summary answer"));
        let done = plain(&panel.render(80).join("\n"));
        assert!(done.contains("summary answer"));
        assert!(!done.contains("starting side agent"));

        panel.mark_failed("late failure");
        let failed = plain(&panel.render(80).join("\n"));
        assert!(failed.contains("late failure"));
        assert!(!panel.is_running());
    }

    #[test]
    fn caps_height_follows_tail_and_scrolls_older_turns() {
        let (mut panel, _, rows) = fixture();
        rows.store(15, Ordering::Relaxed);
        for index in 1..=8 {
            panel.submit(&format!("question {index}"));
            panel.append_answer(&format!("answer {index}"));
            panel.mark_done(None);
        }

        let tail = plain(&panel.render(80).join("\n"));
        assert_eq!(panel.render(80).len(), 5);
        assert!(tail.contains("Esc close · ↑↓ scroll"));
        assert!(tail.contains("question 8"));
        assert!(tail.contains("answer 8"));
        assert!(!tail.contains("question 1"));

        assert!(panel.scroll(BtwScrollDirection::Up));
        let older = plain(&panel.render(80).join("\n"));
        assert_ne!(older, tail);
        assert!(panel.scroll(BtwScrollDirection::Down));

        rows.store(4, Ordering::Relaxed);
        let tiny = panel.render(80);
        assert_eq!(tiny.len(), 3);
        assert!(plain(&tiny.join("\n")).contains("answer 8"));
    }

    #[test]
    fn top_and_body_borders_respect_safe_width() {
        let (mut panel, _, _) = fixture();
        let lines = panel.render(1);
        // The original clamps the panel width to four, then independently
        // clamps body content to one cell. With both side spaces and borders,
        // this deliberately preserves its five-cell body edge case.
        assert_eq!(visible_width(&lines[0]), 4);
        assert_eq!(visible_width(&lines[1]), 5);
        let plain = plain(&lines.join("\n"));
        assert!(plain.starts_with('╭'));
        assert!(plain.contains('╮'));
        assert!(plain.contains('│'));
    }
}
