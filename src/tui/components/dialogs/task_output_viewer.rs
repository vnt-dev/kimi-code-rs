use std::any::Any;

use crate::{
    sdk::types::{BackgroundTaskInfo, BackgroundTaskKind, BackgroundTaskStatus},
    tui::{
        components::{
            Component, ComponentRole,
            render::{truncate_to_width, visible_width},
        },
        keys::{EditorKey, ListKey, matches_editor_key, matches_list_key},
        theme::{ColorToken, current_theme},
        utils::printable_key::printable_char,
    },
};

const ELLIPSIS: &str = "…";

type CloseCallback = dyn FnMut() + Send;
type RowsProvider = dyn Fn() -> usize + Send + Sync;

pub struct TaskOutputViewerProps {
    pub task_id: String,
    pub info: Option<BackgroundTaskInfo>,
    pub output: String,
    on_close: Box<CloseCallback>,
}

impl TaskOutputViewerProps {
    pub fn new<C>(
        task_id: impl Into<String>,
        info: Option<BackgroundTaskInfo>,
        output: impl Into<String>,
        on_close: C,
    ) -> Self
    where
        C: FnMut() + Send + 'static,
    {
        Self {
            task_id: task_id.into(),
            info,
            output: output.into(),
            on_close: Box::new(on_close),
        }
    }
}

/// Full-screen snapshot viewer for one background task's output.
///
/// Original: `task-output-viewer.ts`, `TaskOutputViewer`.
pub struct TaskOutputViewer {
    pub focused: bool,
    props: TaskOutputViewerProps,
    rows: Box<RowsProvider>,
    lines: Vec<String>,
    scroll_top: usize,
}

impl TaskOutputViewer {
    pub fn new<R>(props: TaskOutputViewerProps, rows: R) -> Self
    where
        R: Fn() -> usize + Send + Sync + 'static,
    {
        let lines = split_output(&props.output);
        Self {
            focused: false,
            props,
            rows: Box::new(rows),
            lines,
            scroll_top: 0,
        }
    }

    /// Original: `TaskOutputViewer.setProps()`.
    pub fn set_props(&mut self, next: TaskOutputViewerProps) {
        let output_changed = next.output != self.props.output;
        let was_at_bottom = self.scroll_top >= self.max_scroll();
        self.props = next;
        if output_changed {
            self.lines = split_output(&self.props.output);
            if was_at_bottom {
                self.scroll_top = self.max_scroll();
            } else {
                self.scroll_top = self.scroll_top.min(self.max_scroll());
            }
        }
        self.invalidate();
    }

    pub fn scroll_top(&self) -> usize {
        self.scroll_top
    }

    /// Original: `TaskOutputViewer.handleInput()`.
    pub fn handle_input_event(&mut self, data: &str) {
        let visible = self.viewable_rows();
        let key = printable_char(data);
        if matches_editor_key(data, EditorKey::Escape) || matches!(key.as_str(), "q" | "Q") {
            (self.props.on_close)();
            return;
        }
        if matches_editor_key(data, EditorKey::Up) || key == "k" {
            self.scroll_by(-1);
            return;
        }
        if matches_editor_key(data, EditorKey::Down) || key == "j" {
            self.scroll_by(1);
            return;
        }
        let page = isize::try_from(visible.saturating_sub(1).max(1)).unwrap_or(isize::MAX);
        if matches_list_key(data, ListKey::PageUp)
            || matches_editor_key(data, EditorKey::Ctrl('u'))
            || matches_editor_key(data, EditorKey::Ctrl('b'))
            || key == " "
        {
            self.scroll_by(-page);
            return;
        }
        if matches_list_key(data, ListKey::PageDown)
            || matches_editor_key(data, EditorKey::Ctrl('d'))
            || matches_editor_key(data, EditorKey::Ctrl('f'))
        {
            self.scroll_by(page);
            return;
        }
        if matches_editor_key(data, EditorKey::Home) || key == "g" {
            self.scroll_to(0);
            return;
        }
        if matches_editor_key(data, EditorKey::End) || key == "G" {
            self.scroll_to(self.max_scroll());
        }
    }

    fn scroll_by(&mut self, delta: isize) {
        self.scroll_to(self.scroll_top.saturating_add_signed(delta));
    }

    fn scroll_to(&mut self, target: usize) {
        self.scroll_top = target.min(self.max_scroll());
    }

    fn max_scroll(&self) -> usize {
        self.lines.len().saturating_sub(self.viewable_rows())
    }

    fn viewable_rows(&self) -> usize {
        (self.rows)().saturating_sub(4).max(1)
    }

    /// Original: `TaskOutputViewer.render()`.
    pub fn render_viewer(&mut self, width: usize) -> Vec<String> {
        let body_height = (self.rows)().max(3).saturating_sub(2);
        let header = self.render_header(width);
        let body = self.render_body(width, body_height);
        let footer = self.render_footer(width, body_height);
        std::iter::once(header)
            .chain(body)
            .chain(std::iter::once(footer))
            .collect()
    }

    fn render_header(&self, width: usize) -> String {
        let theme = current_theme();
        let mut composed = format!(
            "{}{}",
            theme.bold_fg(ColorToken::Primary, " Task output "),
            theme.bold_fg(ColorToken::Text, &self.props.task_id)
        );
        if let Some(info) = &self.props.info {
            let mut segments = vec![theme.fg(status_color(info.status), status_label(info.status))];
            if let BackgroundTaskKind::Process {
                exit_code: Some(code),
                ..
            } = info.kind
            {
                segments.push(theme.fg(ColorToken::TextMuted, &format!("exit {code}")));
            }
            if !info.description.is_empty() {
                segments.push(theme.fg(ColorToken::TextMuted, &info.description));
            }
            composed.push_str("  ");
            composed.push_str(&segments.join("  "));
        }
        fit_exactly(&composed, width)
    }

    fn render_body(&mut self, width: usize, body_height: usize) -> Vec<String> {
        let inner_width = width.saturating_sub(4).max(1);
        self.scroll_top = self.scroll_top.min(self.max_scroll());
        let view_rows = body_height.saturating_sub(2);
        let theme = current_theme();
        let top = theme.fg(
            ColorToken::Primary,
            &format!("╭{}╮", "─".repeat(width.saturating_sub(2))),
        );
        let bottom = theme.fg(
            ColorToken::Primary,
            &format!("╰{}╯", "─".repeat(width.saturating_sub(2))),
        );
        let mut output = vec![top];
        for row in 0..view_rows {
            let raw = self
                .lines
                .get(self.scroll_top + row)
                .map_or("", String::as_str);
            output.push(format!(
                "{}{}{}",
                theme.fg(ColorToken::Primary, "│"),
                fit_exactly(&theme.fg(ColorToken::Text, raw), inner_width),
                theme.fg(ColorToken::Primary, " │")
            ));
        }
        output.push(bottom);
        output
    }

    fn render_footer(&self, width: usize, body_height: usize) -> String {
        let theme = current_theme();
        let total = self.lines.len();
        let view_rows = body_height.saturating_sub(2).max(1);
        let max_scroll = total.saturating_sub(view_rows);
        let percent = if max_scroll == 0 {
            100
        } else {
            ((self.scroll_top as f64 / max_scroll as f64) * 100.0).round() as usize
        };
        let position = theme.fg(
            ColorToken::TextMuted,
            &format!(
                " {}-{} / {total} ({percent}%) ",
                self.scroll_top + 1,
                total.min(self.scroll_top + view_rows)
            ),
        );
        let key = |text: &str| theme.bold_fg(ColorToken::Primary, text);
        let dim = |text: &str| theme.fg(ColorToken::TextMuted, text);
        let left = format!(
            " {} {}  {} {}  {} {}  {} {}",
            key("↑↓"),
            dim("line"),
            key("PgUp/PgDn/Ctrl+U/D"),
            dim("page"),
            key("g/G"),
            dim("top/bot"),
            key("Q/Esc"),
            dim("cancel")
        );
        let left_width = visible_width(&left);
        let right_width = visible_width(&position);
        if left_width + 2 + right_width <= width {
            format!(
                "{left}{}{position}",
                " ".repeat(width - left_width - right_width)
            )
        } else {
            fit_exactly(&left, width)
        }
    }
}

impl Component for TaskOutputViewer {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_viewer(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.handle_input_event(data);
    }

    fn invalidate(&mut self) {}

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn split_output(output: &str) -> Vec<String> {
    if output.is_empty() {
        vec!["[no output captured]".to_owned()]
    } else {
        output.split('\n').map(str::to_owned).collect()
    }
}

fn status_label(status: BackgroundTaskStatus) -> &'static str {
    match status {
        BackgroundTaskStatus::Running => "running",
        BackgroundTaskStatus::Completed => "completed",
        BackgroundTaskStatus::Failed => "failed",
        BackgroundTaskStatus::TimedOut => "timed out",
        BackgroundTaskStatus::Killed => "killed",
        BackgroundTaskStatus::Lost => "lost",
    }
}

fn status_color(status: BackgroundTaskStatus) -> ColorToken {
    match status {
        BackgroundTaskStatus::Running => ColorToken::Success,
        BackgroundTaskStatus::Completed => ColorToken::TextMuted,
        BackgroundTaskStatus::Failed
        | BackgroundTaskStatus::TimedOut
        | BackgroundTaskStatus::Killed
        | BackgroundTaskStatus::Lost => ColorToken::Error,
    }
}

fn pad_to_width(line: &str, width: usize) -> String {
    let current = visible_width(line);
    if current == width {
        line.to_owned()
    } else if current > width {
        truncate_to_width(line, width, ELLIPSIS, false)
    } else {
        format!("{line}{}", " ".repeat(width - current))
    }
}

fn fit_exactly(line: &str, width: usize) -> String {
    let line = if visible_width(line) > width {
        truncate_to_width(line, width, ELLIPSIS, false)
    } else {
        line.to_owned()
    };
    pad_to_width(&line, width)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    fn props(output: &str, closes: Arc<Mutex<usize>>) -> TaskOutputViewerProps {
        TaskOutputViewerProps::new("task-1", None, output, move || {
            *closes.lock().expect("close count") += 1;
        })
    }

    fn plain(text: &str) -> String {
        let ansi = regex::Regex::new("\\x1b\\[[0-9;]*m").expect("ANSI regex");
        ansi.replace_all(text, "").into_owned()
    }

    #[test]
    fn renders_no_output_and_process_status_header_at_terminal_height() {
        let closes = Arc::new(Mutex::new(0));
        let mut empty = TaskOutputViewer::new(props("", closes), || 8);
        assert!(plain(&empty.render_viewer(80).join("\n")).contains("[no output captured]"));

        let info = BackgroundTaskInfo {
            task_id: "task-1".to_owned(),
            description: "build project".to_owned(),
            status: BackgroundTaskStatus::Failed,
            detached: Some(true),
            started_at: 1.0,
            ended_at: Some(2.0),
            stop_reason: None,
            terminal_notification_suppressed: None,
            timeout_ms: None,
            kind: BackgroundTaskKind::Process {
                command: "cargo build".to_owned(),
                pid: 10,
                exit_code: Some(2),
            },
        };
        let mut viewer = TaskOutputViewer::new(
            TaskOutputViewerProps::new("task-1", Some(info), "line", || {}),
            || 8,
        );
        let lines = viewer.render_viewer(80);
        assert_eq!(lines.len(), 8);
        let header = plain(&lines[0]);
        assert!(header.contains("failed"));
        assert!(header.contains("exit 2"));
        assert!(header.contains("build project"));
    }

    #[test]
    fn follows_tail_only_when_previously_at_bottom() {
        let closes = Arc::new(Mutex::new(0));
        let mut viewer = TaskOutputViewer::new(
            props(
                &(1..=20)
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
                Arc::clone(&closes),
            ),
            || 8,
        );
        viewer.handle_input_event("G");
        assert_eq!(viewer.scroll_top(), 16);
        viewer.set_props(props(
            &(1..=25)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
            Arc::clone(&closes),
        ));
        assert_eq!(viewer.scroll_top(), 21);
        viewer.handle_input_event("k");
        viewer.set_props(props(
            &(1..=30)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
            closes,
        ));
        assert_eq!(viewer.scroll_top(), 20);
    }

    #[test]
    fn navigation_and_dynamic_rows_clamp_like_less() {
        let rows = Arc::new(AtomicUsize::new(9));
        let source = Arc::clone(&rows);
        let closes = Arc::new(Mutex::new(0));
        let mut viewer = TaskOutputViewer::new(
            props(
                &(1..=30)
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
                closes,
            ),
            move || source.load(Ordering::Relaxed),
        );
        viewer.handle_input_event("G");
        assert_eq!(viewer.scroll_top(), 25);
        viewer.handle_input_event("\u{15}");
        assert_eq!(viewer.scroll_top(), 21);
        viewer.handle_input_event("\u{4}");
        assert_eq!(viewer.scroll_top(), 25);
        rows.store(14, Ordering::Relaxed);
        viewer.handle_input_event("G");
        assert_eq!(viewer.scroll_top(), 20);
    }

    #[test]
    fn q_and_escape_close_and_footer_reports_bottom() {
        for key in ["q", "Q", "\u{1b}"] {
            let closes = Arc::new(Mutex::new(0));
            let mut viewer = TaskOutputViewer::new(props("one\ntwo", Arc::clone(&closes)), || 8);
            viewer.handle_input_event(key);
            assert_eq!(*closes.lock().expect("close count"), 1);
        }
        let closes = Arc::new(Mutex::new(0));
        let mut viewer = TaskOutputViewer::new(props("one\ntwo", closes), || 8);
        let footer = plain(viewer.render_viewer(120).last().expect("footer"));
        assert!(footer.contains("1-2 / 2 (100%)"));
    }
}
