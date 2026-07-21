use std::{
    any::Any,
    cmp::Ordering,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    sdk::types::{BackgroundTaskInfo, BackgroundTaskKind, BackgroundTaskStatus},
    tui::{
        components::{
            Component, ComponentRole,
            render::{truncate_to_width, visible_width},
        },
        keys::{EditorKey, matches_editor_key},
        theme::{ColorToken, current_theme},
        utils::printable_key::printable_char,
    },
};

const MIN_WIDTH: usize = 48;
const MIN_HEIGHT: usize = 10;
const LIST_COL_MIN: usize = 28;
const LIST_COL_MAX: usize = 44;
const STOP_CONFIRM_TIMEOUT: Duration = Duration::from_secs(5);
const SELECT_POINTER: &str = "❯";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TasksFilter {
    All,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopIgnoredReason {
    Terminal,
}

type IdCallback = dyn FnMut(String) + Send;
type VoidCallback = dyn FnMut() + Send;
type StopIgnoredCallback = dyn FnMut(String, StopIgnoredReason) + Send;

pub struct TasksBrowserProps {
    pub tasks: Vec<BackgroundTaskInfo>,
    pub filter: TasksFilter,
    pub selected_task_id: Option<String>,
    pub tail_output: Option<String>,
    pub tail_loading: bool,
    pub flash_message: Option<String>,
    pub terminal_rows: usize,
    on_select: Box<IdCallback>,
    on_toggle_filter: Box<VoidCallback>,
    on_refresh: Box<VoidCallback>,
    on_cancel: Box<VoidCallback>,
    on_stop_confirmed: Box<IdCallback>,
    on_open_output: Box<IdCallback>,
    on_stop_ignored: Option<Box<StopIgnoredCallback>>,
}

impl TasksBrowserProps {
    #[allow(clippy::too_many_arguments)]
    pub fn new<S, T, R, C, K, O>(
        tasks: Vec<BackgroundTaskInfo>,
        terminal_rows: usize,
        on_select: S,
        on_toggle_filter: T,
        on_refresh: R,
        on_cancel: C,
        on_stop_confirmed: K,
        on_open_output: O,
    ) -> Self
    where
        S: FnMut(String) + Send + 'static,
        T: FnMut() + Send + 'static,
        R: FnMut() + Send + 'static,
        C: FnMut() + Send + 'static,
        K: FnMut(String) + Send + 'static,
        O: FnMut(String) + Send + 'static,
    {
        Self {
            tasks,
            filter: TasksFilter::All,
            selected_task_id: None,
            tail_output: None,
            tail_loading: false,
            flash_message: None,
            terminal_rows,
            on_select: Box::new(on_select),
            on_toggle_filter: Box::new(on_toggle_filter),
            on_refresh: Box::new(on_refresh),
            on_cancel: Box::new(on_cancel),
            on_stop_confirmed: Box::new(on_stop_confirmed),
            on_open_output: Box::new(on_open_output),
            on_stop_ignored: None,
        }
    }

    pub fn with_stop_ignored<I>(mut self, callback: I) -> Self
    where
        I: FnMut(String, StopIgnoredReason) + Send + 'static,
    {
        self.on_stop_ignored = Some(Box::new(callback));
        self
    }
}

/// Full-screen three-pane background-task browser.
///
/// Original: `tasks-browser.ts`, `TasksBrowserApp`.
pub struct TasksBrowserApp {
    pub focused: bool,
    props: TasksBrowserProps,
    sorted_visible: Vec<BackgroundTaskInfo>,
    selected_index: usize,
    list_scroll: usize,
    pending_stop: Option<(String, Instant)>,
}

impl TasksBrowserApp {
    pub fn new(props: TasksBrowserProps) -> Self {
        let sorted_visible = sorted_visible(&props.tasks, props.filter);
        let mut app = Self {
            focused: false,
            props,
            sorted_visible,
            selected_index: 0,
            list_scroll: 0,
            pending_stop: None,
        };
        app.sync_selection_from_props();
        app
    }

    pub fn set_props(&mut self, props: TasksBrowserProps) {
        self.props = props;
        self.sorted_visible = sorted_visible(&self.props.tasks, self.props.filter);
        self.sync_selection_from_props();
        if self.pending_stop.as_ref().is_some_and(|(id, _)| {
            self.props
                .tasks
                .iter()
                .find(|task| &task.task_id == id)
                .is_none_or(|task| is_terminal(task.status))
        }) {
            self.pending_stop = None;
        }
    }

    pub fn selected_task_id(&self) -> Option<&str> {
        self.sorted_visible
            .get(self.selected_index)
            .map(|task| task.task_id.as_str())
    }

    fn sync_selection_from_props(&mut self) {
        if self.sorted_visible.is_empty() {
            self.selected_index = 0;
            self.list_scroll = 0;
            return;
        }
        if let Some(index) = self.props.selected_task_id.as_deref().and_then(|id| {
            self.sorted_visible
                .iter()
                .position(|task| task.task_id == id)
        }) {
            self.selected_index = index;
            return;
        }
        self.selected_index = self.selected_index.min(self.sorted_visible.len() - 1);
    }

    fn expire_pending_stop(&mut self) {
        if self
            .pending_stop
            .as_ref()
            .is_some_and(|(_, started)| started.elapsed() >= STOP_CONFIRM_TIMEOUT)
        {
            self.pending_stop = None;
        }
    }

    pub fn handle_input_event(&mut self, data: &str) {
        self.expire_pending_stop();
        let key = printable_char(data);
        if self.pending_stop.is_some() {
            let pending = self.pending_stop.take();
            if matches!(key.as_str(), "y" | "Y")
                && let Some((id, _)) = pending
            {
                (self.props.on_stop_confirmed)(id);
            }
            return;
        }
        if matches_editor_key(data, EditorKey::Escape) || matches!(key.as_str(), "q" | "Q") {
            (self.props.on_cancel)();
            return;
        }
        if matches_editor_key(data, EditorKey::Up) || key == "k" {
            self.move_selection(-1);
            return;
        }
        if matches_editor_key(data, EditorKey::Down) || key == "j" {
            self.move_selection(1);
            return;
        }
        if matches_editor_key(data, EditorKey::Tab) {
            (self.props.on_toggle_filter)();
            return;
        }
        if matches!(key.as_str(), "r" | "R") {
            (self.props.on_refresh)();
            return;
        }
        if matches!(key.as_str(), "s" | "S") {
            let Some(task) = self.sorted_visible.get(self.selected_index) else {
                return;
            };
            if is_terminal(task.status) {
                if let Some(callback) = &mut self.props.on_stop_ignored {
                    callback(task.task_id.clone(), StopIgnoredReason::Terminal);
                }
            } else {
                self.pending_stop = Some((task.task_id.clone(), Instant::now()));
            }
            return;
        }
        if (matches!(key.as_str(), "o" | "O") || matches_editor_key(data, EditorKey::Enter))
            && let Some(task) = self.sorted_visible.get(self.selected_index)
        {
            (self.props.on_open_output)(task.task_id.clone());
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.sorted_visible.is_empty() {
            return;
        }
        self.selected_index = self
            .selected_index
            .saturating_add_signed(delta)
            .min(self.sorted_visible.len() - 1);
        (self.props.on_select)(self.sorted_visible[self.selected_index].task_id.clone());
    }

    fn render_app(&mut self, width: usize) -> Vec<String> {
        self.expire_pending_stop();
        let rows = self.props.terminal_rows.max(1);
        if width < MIN_WIDTH || rows < MIN_HEIGHT {
            return self.render_too_small(width, rows);
        }
        let body_height = rows - 2;
        let list_width = ((width as f64 * 0.32).floor() as usize).clamp(LIST_COL_MIN, LIST_COL_MAX);
        let right_width = width - list_width;
        let list = self.render_list_frame(list_width, body_height);
        let right = self.render_right_stack(right_width, body_height);
        let mut lines = vec![self.render_header(width)];
        for index in 0..body_height {
            lines.push(format!(
                "{}{}",
                list.get(index)
                    .cloned()
                    .unwrap_or_else(|| " ".repeat(list_width)),
                right
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| " ".repeat(right_width))
            ));
        }
        lines.push(self.render_footer(width));
        lines
    }

    fn render_header(&self, width: usize) -> String {
        let visible = visible_tasks(&self.props.tasks, self.props.filter);
        let counts = count_by_status(&visible);
        let mut line = format!(
            "{}{}",
            current_theme().bold_fg(ColorToken::Primary, " TASK BROWSER "),
            current_theme().fg(
                ColorToken::TextMuted,
                &format!(
                    " filter={} ",
                    if self.props.filter == TasksFilter::All {
                        "ALL"
                    } else {
                        "ACTIVE"
                    }
                )
            )
        );
        if counts.0 > 0 {
            line.push_str(
                &current_theme().fg(ColorToken::Success, &format!(" {} running ", counts.0)),
            );
        }
        if counts.1 > 0 {
            line.push_str(
                &current_theme().fg(ColorToken::TextDim, &format!(" {} completed ", counts.1)),
            );
        }
        if counts.2 > 0 {
            line.push_str(
                &current_theme().fg(ColorToken::Error, &format!(" {} interrupted ", counts.2)),
            );
        }
        line.push_str(
            &current_theme().fg(ColorToken::TextMuted, &format!(" {} total ", visible.len())),
        );
        fit_exactly(&line, width)
    }

    fn render_footer(&self, width: usize) -> String {
        if let Some((id, _)) = &self.pending_stop {
            return fit_exactly(
                &format!(
                    " {} {}? {} {}  {}/{} {} ",
                    current_theme().bold_fg(ColorToken::Warning, "Stop"),
                    current_theme().fg(ColorToken::Text, id),
                    current_theme().bold_fg(ColorToken::Primary, "Y"),
                    current_theme().fg(ColorToken::TextMuted, "confirm"),
                    current_theme().bold_fg(ColorToken::Primary, "N"),
                    current_theme().bold_fg(ColorToken::Primary, "esc"),
                    current_theme().fg(ColorToken::TextMuted, "cancel")
                ),
                width,
            );
        }
        let line = format!(
            " {} {}  {} {}  {} {}  {} {}  {} {}  {} {} ",
            key("↑↓"),
            dim("select"),
            key("Enter/O"),
            dim("output"),
            key("S"),
            dim("stop"),
            key("R"),
            dim("refresh"),
            key("Tab"),
            dim("filter"),
            key("Q/Esc"),
            dim("cancel")
        );
        if let Some(flash) = self
            .props
            .flash_message
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            let flash = current_theme().fg(ColorToken::Warning, &format!(" {flash} "));
            let total = visible_width(&line) + visible_width(&flash);
            if total <= width {
                return format!("{line}{}{flash}", " ".repeat(width - total));
            }
        }
        fit_exactly(&line, width)
    }

    fn render_frame(
        &self,
        title: &str,
        content: &[String],
        width: usize,
        height: usize,
    ) -> Vec<String> {
        if height < 2 || width < 4 {
            return vec![" ".repeat(width); height];
        }
        let inner_width = width - 2;
        let inner_height = height - 2;
        let title = current_theme().bold_fg(ColorToken::TextStrong, title);
        let segment_width = visible_width(&format!("─ {title} "));
        let middle = if !title.is_empty() && segment_width <= inner_width {
            format!(
                "{}{} {}",
                current_theme().fg(ColorToken::Primary, "─ "),
                title,
                current_theme().fg(
                    ColorToken::Primary,
                    &"─".repeat(inner_width - segment_width)
                )
            )
        } else {
            current_theme().fg(ColorToken::Primary, &"─".repeat(inner_width))
        };
        let mut lines = vec![format!(
            "{}{middle}{}",
            current_theme().fg(ColorToken::Primary, "┌"),
            current_theme().fg(ColorToken::Primary, "┐")
        )];
        for index in 0..inner_height {
            lines.push(format!(
                "{}{}{}",
                current_theme().fg(ColorToken::Primary, "│"),
                fit_exactly(content.get(index).map_or("", String::as_str), inner_width),
                current_theme().fg(ColorToken::Primary, "│")
            ));
        }
        lines.push(current_theme().fg(
            ColorToken::Primary,
            &format!("└{}┘", "─".repeat(inner_width)),
        ));
        lines
    }

    fn render_list_frame(&mut self, width: usize, height: usize) -> Vec<String> {
        let inner_height = height.saturating_sub(2);
        let title = format!(
            "Tasks [{}]",
            if self.props.filter == TasksFilter::All {
                "all"
            } else {
                "active"
            }
        );
        if self.sorted_visible.is_empty() {
            let empty = if self.props.filter == TasksFilter::Active {
                "No active tasks. Tab = show all."
            } else {
                "No background tasks in this session."
            };
            return self.render_frame(
                &title,
                &[current_theme().fg(ColorToken::TextMuted, empty)],
                width,
                height,
            );
        }
        self.adjust_scroll(inner_height);
        let inner_width = width - 2;
        let mut content = Vec::new();
        for index in
            self.list_scroll..(self.list_scroll + inner_height).min(self.sorted_visible.len())
        {
            content.push(render_list_row(
                &self.sorted_visible[index],
                index == self.selected_index,
                inner_width,
            ));
        }
        self.render_frame(&title, &content, width, height)
    }

    fn adjust_scroll(&mut self, rows: usize) {
        if rows == 0 {
            self.list_scroll = 0;
            return;
        }
        if self.selected_index < self.list_scroll {
            self.list_scroll = self.selected_index;
        } else if self.selected_index >= self.list_scroll + rows {
            self.list_scroll = self.selected_index - rows + 1;
        }
        self.list_scroll = self
            .list_scroll
            .min(self.sorted_visible.len().saturating_sub(rows));
    }

    fn render_right_stack(&self, width: usize, height: usize) -> Vec<String> {
        let detail_height =
            8.max(((height as f64 * 0.4).floor() as usize).min(height.saturating_sub(5)));
        let preview_height = height.saturating_sub(detail_height);
        [
            self.render_detail_frame(width, detail_height),
            self.render_preview_frame(width, preview_height),
        ]
        .concat()
    }

    fn render_detail_frame(&self, width: usize, height: usize) -> Vec<String> {
        let Some(task) = self.sorted_visible.get(self.selected_index) else {
            return self.render_frame(
                "Detail",
                &[current_theme().fg(ColorToken::TextMuted, "Select a task from the list.")],
                width,
                height,
            );
        };
        let mut lines = vec![
            detail("Task ID:", &task.task_id, ColorToken::Text),
            detail(
                "Status:",
                status_label(task.status),
                status_color(task.status),
            ),
            detail(
                "Description:",
                &if task.description.trim().is_empty() {
                    "—".to_owned()
                } else {
                    single_line(&task.description)
                },
                ColorToken::Text,
            ),
        ];
        match &task.kind {
            BackgroundTaskKind::Process {
                command,
                pid,
                exit_code,
            } => {
                if !command.is_empty() && command != &task.description {
                    lines.push(detail("Command:", &single_line(command), ColorToken::Text));
                }
                if *pid > 0 {
                    lines.push(detail("Pid:", &pid.to_string(), ColorToken::TextMuted));
                }
                if let Some(code) = exit_code {
                    lines.push(detail(
                        "Exit code:",
                        &code.to_string(),
                        ColorToken::TextMuted,
                    ));
                }
            }
            BackgroundTaskKind::Agent {
                agent_id,
                subagent_type,
            } => {
                if let Some(id) = agent_id {
                    lines.push(detail("Agent ID:", id, ColorToken::Text));
                }
                if let Some(kind) = subagent_type {
                    lines.push(detail("Agent type:", kind, ColorToken::Text));
                }
            }
            BackgroundTaskKind::Question {
                question_count,
                tool_call_id,
            } => {
                lines.push(detail(
                    "Questions:",
                    &question_count.to_string(),
                    ColorToken::TextMuted,
                ));
                if let Some(id) = tool_call_id {
                    lines.push(detail("Tool call:", id, ColorToken::TextMuted));
                }
            }
        }
        let timestamp = if task.status == BackgroundTaskStatus::Running {
            Some(("running", task.started_at))
        } else {
            task.ended_at.map(|time| ("finished", time))
        };
        if let Some((verb, time)) = timestamp {
            let relative = format_relative_time(time, now_millis());
            if !relative.is_empty() {
                lines.push(detail(
                    "Time:",
                    &format!("{verb} {relative}"),
                    ColorToken::TextMuted,
                ));
            }
        }
        if let Some(reason) = task
            .stop_reason
            .as_deref()
            .filter(|reason| !reason.is_empty())
        {
            lines.push(detail("Reason:", reason, ColorToken::TextMuted));
        }
        self.render_frame("Detail", &lines, width, height)
    }

    fn render_preview_frame(&self, width: usize, height: usize) -> Vec<String> {
        if self.sorted_visible.get(self.selected_index).is_none() {
            return self.render_frame(
                "Preview Output",
                &[current_theme().fg(ColorToken::TextMuted, "No task selected.")],
                width,
                height,
            );
        }
        let body = if self.props.tail_loading {
            "[loading…]"
        } else {
            self.props
                .tail_output
                .as_deref()
                .filter(|text| !text.is_empty())
                .unwrap_or("[no output captured]")
        };
        let inner_height = height.saturating_sub(2);
        let raw = body.split('\n').collect::<Vec<_>>();
        let start = raw.len().saturating_sub(inner_height);
        let lines = raw[start..]
            .iter()
            .map(|line| current_theme().fg(ColorToken::TextDim, line))
            .collect::<Vec<_>>();
        self.render_frame("Preview Output", &lines, width, height)
    }

    fn render_too_small(&self, width: usize, rows: usize) -> Vec<String> {
        let mut lines = vec![fit_exactly(
            &current_theme().fg(
                ColorToken::Error,
                &format!("Terminal too small (need ≥{MIN_WIDTH} × {MIN_HEIGHT})"),
            ),
            width,
        )];
        lines.resize(rows, " ".repeat(width));
        lines
    }
}

impl Component for TasksBrowserApp {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_app(width)
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

fn visible_tasks(tasks: &[BackgroundTaskInfo], filter: TasksFilter) -> Vec<BackgroundTaskInfo> {
    tasks
        .iter()
        .filter(|task| task.detached != Some(false))
        .filter(|task| filter == TasksFilter::All || !is_terminal(task.status))
        .cloned()
        .collect()
}
fn sorted_visible(tasks: &[BackgroundTaskInfo], filter: TasksFilter) -> Vec<BackgroundTaskInfo> {
    let mut tasks = visible_tasks(tasks, filter);
    tasks.sort_by(compare_tasks);
    tasks
}
fn compare_tasks(a: &BackgroundTaskInfo, b: &BackgroundTaskInfo) -> Ordering {
    match (is_terminal(a.status), is_terminal(b.status)) {
        (false, true) => Ordering::Less,
        (true, false) => Ordering::Greater,
        (false, false) => a
            .started_at
            .partial_cmp(&b.started_at)
            .unwrap_or(Ordering::Equal),
        (true, true) => b
            .ended_at
            .unwrap_or(b.started_at)
            .partial_cmp(&a.ended_at.unwrap_or(a.started_at))
            .unwrap_or(Ordering::Equal),
    }
}
fn is_terminal(status: BackgroundTaskStatus) -> bool {
    status != BackgroundTaskStatus::Running
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
        _ => ColorToken::Error,
    }
}
fn count_by_status(tasks: &[BackgroundTaskInfo]) -> (usize, usize, usize) {
    let mut counts = (0, 0, 0);
    for task in tasks {
        match task.status {
            BackgroundTaskStatus::Running => counts.0 += 1,
            BackgroundTaskStatus::Completed => counts.1 += 1,
            _ => counts.2 += 1,
        }
    }
    counts
}
fn render_list_row(task: &BackgroundTaskInfo, selected: bool, width: usize) -> String {
    let pointer_text = if selected {
        format!("{SELECT_POINTER} ")
    } else {
        "  ".to_owned()
    };
    let pointer = current_theme().fg(
        if selected {
            ColorToken::Primary
        } else {
            ColorToken::TextDim
        },
        &pointer_text,
    );
    let kind_color = if selected {
        ColorToken::Primary
    } else {
        match task.kind {
            BackgroundTaskKind::Agent { .. } => ColorToken::Success,
            BackgroundTaskKind::Question { .. } => ColorToken::Warning,
            BackgroundTaskKind::Process { .. } => ColorToken::Accent,
        }
    };
    let id = if selected {
        current_theme().bold_fg(kind_color, &task.task_id)
    } else {
        current_theme().fg(kind_color, &task.task_id)
    };
    let padding = " ".repeat(17usize.saturating_sub(task.task_id.len()));
    let prefix = format!(
        "{pointer}{id}{padding} {}",
        current_theme().fg(status_color(task.status), status_label(task.status))
    );
    let budget = width.saturating_sub(visible_width(&prefix) + 1);
    if budget < 4 {
        return fit_exactly(&prefix, width);
    }
    let fallback = match &task.kind {
        BackgroundTaskKind::Process { command, .. } => command.as_str(),
        _ => "",
    };
    let description = if task.description.trim().is_empty() {
        fallback
    } else {
        &task.description
    };
    let description = if description.trim().is_empty() {
        "(no description)".to_owned()
    } else {
        single_line(description)
    };
    fit_exactly(
        &format!(
            "{prefix} {}",
            current_theme().fg(
                ColorToken::Text,
                &truncate_to_width(&description, budget, "…", false)
            )
        ),
        width,
    )
}
fn detail(label: &str, value: &str, token: ColorToken) -> String {
    format!(
        "{}{}",
        current_theme().fg(ColorToken::TextMuted, &format!("{label:<14}")),
        current_theme().fg(token, value)
    )
}
fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn fit_exactly(line: &str, width: usize) -> String {
    let line = truncate_to_width(line, width, "…", false);
    format!(
        "{line}{}",
        " ".repeat(width.saturating_sub(visible_width(&line)))
    )
}
fn key(text: &str) -> String {
    current_theme().bold_fg(ColorToken::Primary, text)
}
fn dim(text: &str) -> String {
    current_theme().fg(ColorToken::TextMuted, text)
}
fn now_millis() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |time| time.as_secs_f64() * 1000.0)
}
fn format_relative_time(timestamp: f64, now: f64) -> String {
    if !timestamp.is_finite() || timestamp <= 0.0 {
        return String::new();
    }
    let seconds = ((now - timestamp).max(0.0) / 1000.0).floor() as u64;
    if seconds < 60 {
        "just now".to_owned()
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    fn task(
        id: &str,
        status: BackgroundTaskStatus,
        detached: Option<bool>,
        started: f64,
    ) -> BackgroundTaskInfo {
        BackgroundTaskInfo {
            task_id: id.to_owned(),
            description: format!("Task {id}"),
            status,
            detached,
            started_at: started,
            ended_at: is_terminal(status).then_some(started + 10.0),
            stop_reason: None,
            terminal_notification_suppressed: None,
            timeout_ms: None,
            kind: BackgroundTaskKind::Process {
                command: format!("echo {id}"),
                pid: 10,
                exit_code: None,
            },
        }
    }
    fn props(tasks: Vec<BackgroundTaskInfo>, events: Arc<Mutex<Vec<String>>>) -> TasksBrowserProps {
        let a = Arc::clone(&events);
        let b = Arc::clone(&events);
        let c = Arc::clone(&events);
        let d = Arc::clone(&events);
        let e = Arc::clone(&events);
        let f = Arc::clone(&events);
        TasksBrowserProps::new(
            tasks,
            18,
            move |id| a.lock().expect("events").push(format!("select:{id}")),
            move || b.lock().expect("events").push("filter".into()),
            move || c.lock().expect("events").push("refresh".into()),
            move || d.lock().expect("events").push("cancel".into()),
            move |id| e.lock().expect("events").push(format!("stop:{id}")),
            move |id| f.lock().expect("events").push(format!("open:{id}")),
        )
    }
    #[test]
    fn filters_foreground_and_sorts_running_then_recent_terminal() {
        let tasks = vec![
            task("old", BackgroundTaskStatus::Completed, None, 1.0),
            task("run2", BackgroundTaskStatus::Running, None, 3.0),
            task("fg", BackgroundTaskStatus::Running, Some(false), 0.0),
            task("run1", BackgroundTaskStatus::Running, None, 2.0),
            task("new", BackgroundTaskStatus::Failed, None, 5.0),
        ];
        assert_eq!(
            sorted_visible(&tasks, TasksFilter::All)
                .iter()
                .map(|t| t.task_id.as_str())
                .collect::<Vec<_>>(),
            ["run1", "run2", "new", "old"]
        );
        assert_eq!(visible_tasks(&tasks, TasksFilter::Active).len(), 2);
    }
    #[test]
    fn dispatches_navigation_output_controls_and_stop_confirmation() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut app = TasksBrowserApp::new(props(
            vec![
                task("a", BackgroundTaskStatus::Running, None, 1.0),
                task("b", BackgroundTaskStatus::Completed, None, 2.0),
            ],
            Arc::clone(&events),
        ));
        app.handle_input_event("j");
        app.handle_input_event("\r");
        app.handle_input_event("r");
        app.handle_input_event("\t");
        app.handle_input_event("k");
        app.handle_input_event("s");
        app.handle_input_event("y");
        app.handle_input_event("q");
        assert_eq!(
            *events.lock().expect("events"),
            [
                "select:b", "open:b", "refresh", "filter", "select:a", "stop:a", "cancel"
            ]
        );
    }
    #[test]
    fn ignores_stop_for_terminal_and_clears_pending_on_other_key() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let ignored = Arc::clone(&events);
        let p = props(
            vec![task("done", BackgroundTaskStatus::Completed, None, 1.0)],
            Arc::clone(&events),
        )
        .with_stop_ignored(move |id, _| {
            ignored
                .lock()
                .expect("events")
                .push(format!("ignored:{id}"))
        });
        let mut app = TasksBrowserApp::new(p);
        app.handle_input_event("s");
        assert_eq!(*events.lock().expect("events"), ["ignored:done"]);
    }
    #[test]
    fn renders_exact_full_screen_and_too_small_fallback() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut p = props(
            vec![task("a", BackgroundTaskStatus::Running, None, now_millis())],
            events,
        );
        p.tail_output = Some("one\ntwo".into());
        let mut app = TasksBrowserApp::new(p);
        let lines = app.render(90);
        assert_eq!(lines.len(), 18);
        assert!(lines.iter().all(|line| visible_width(line) == 90));
        let small = app.render(40);
        assert_eq!(small.len(), 18);
        assert!(strip(&small[0]).contains("too small"));
    }
    fn strip(text: &str) -> String {
        regex::Regex::new(r"\x1b\[[0-9;]*m")
            .expect("regex")
            .replace_all(text, "")
            .into_owned()
    }
}
