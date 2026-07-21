use std::{
    any::Any,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::tui::{
    components::{
        Component, ComponentRole, Text,
        render::{truncate_to_width, visible_width},
    },
    theme::{ColorToken, current_theme},
    utils::render_cache::is_render_cache_enabled,
};

const MESSAGE_INDENT: &str = "  ";
const STATUS_BULLET: &str = "● ";
const THINKING_PREVIEW_LINES: usize = 2;
const BRAILLE_SPINNER_INTERVAL: Duration = Duration::from_millis(80);
const BRAILLE_SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThinkingRenderMode {
    Live,
    #[default]
    Finalized,
}

struct SpinnerWorker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl SpinnerWorker {
    fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

/// Live/finalized reasoning transcript with expandable previews.
///
/// Original:
/// `src/tui/components/messages/thinking.ts`, `ThinkingComponent`.
pub struct ThinkingComponent {
    text: String,
    show_marker: bool,
    mode: ThinkingRenderMode,
    expanded: bool,
    spinner_frame: Arc<AtomicUsize>,
    spinner_worker: Option<SpinnerWorker>,
    request_render: Option<Arc<dyn Fn() + Send + Sync>>,
    text_component: Text,
    render_cache: Option<(usize, usize, Vec<String>)>,
}

impl ThinkingComponent {
    pub fn new(
        text: impl Into<String>,
        show_marker: bool,
        mode: ThinkingRenderMode,
        request_render: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        let text = text.into();
        let mut component = Self {
            text_component: Text::new(styled(&text), 0, 0),
            text,
            show_marker,
            mode,
            expanded: false,
            spinner_frame: Arc::new(AtomicUsize::new(0)),
            spinner_worker: None,
            request_render,
            render_cache: None,
        };
        if mode == ThinkingRenderMode::Live {
            component.start_spinner();
        }
        component
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        let text = text.into();
        if self.text != text {
            self.text = text;
            self.mark_render_dirty();
            self.text_component.set_text(styled(&self.text));
        }
    }

    pub fn finalize(&mut self) {
        self.mode = ThinkingRenderMode::Finalized;
        self.mark_render_dirty();
        self.stop_spinner();
    }

    pub fn dispose(&mut self) {
        self.stop_spinner();
    }

    pub fn set_expanded(&mut self, expanded: bool) {
        if self.expanded != expanded {
            self.expanded = expanded;
            self.mark_render_dirty();
        }
    }

    /// Advances one frame for runtimes that provide their own timer.
    pub fn advance_spinner(&mut self) {
        advance_frame(&self.spinner_frame);
        self.mark_render_dirty();
        if let Some(request_render) = &self.request_render {
            request_render();
        }
    }

    fn mark_render_dirty(&mut self) {
        self.render_cache = None;
    }

    fn start_spinner(&mut self) {
        if self.request_render.is_none() || self.spinner_worker.is_some() {
            return;
        }
        let frame = Arc::clone(&self.spinner_frame);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let request_render = Arc::clone(self.request_render.as_ref().expect("checked above"));
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                thread::park_timeout(BRAILLE_SPINNER_INTERVAL);
                if thread_stop.load(Ordering::Acquire) {
                    break;
                }
                advance_frame(&frame);
                request_render();
            }
        });
        self.spinner_worker = Some(SpinnerWorker {
            stop,
            handle: Some(handle),
        });
    }

    fn stop_spinner(&mut self) {
        if let Some(worker) = self.spinner_worker.take() {
            worker.stop();
        }
    }
}

impl Component for ThinkingComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let spinner_frame = self.spinner_frame.load(Ordering::Relaxed);
        if is_render_cache_enabled()
            && let Some((cached_width, cached_frame, lines)) = &self.render_cache
            && *cached_width == width
            && *cached_frame == spinner_frame
        {
            return lines.clone();
        }

        let content_width = width.saturating_sub(visible_width(MESSAGE_INDENT)).max(1);
        let content_lines = if self.text.is_empty() {
            vec![String::new()]
        } else {
            self.text_component.render(content_width)
        };
        let rendered = match self.mode {
            ThinkingRenderMode::Live => self.render_live(width, &content_lines),
            ThinkingRenderMode::Finalized => self.render_finalized(width, &content_lines),
        };
        if is_render_cache_enabled() {
            self.render_cache = Some((width, spinner_frame, rendered.clone()));
        }
        rendered
    }

    fn invalidate(&mut self) {
        self.mark_render_dirty();
        self.text_component.set_text(styled(&self.text));
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::Thinking
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl ThinkingComponent {
    fn render_live(&self, width: usize, content_lines: &[String]) -> Vec<String> {
        let start = content_lines.len().saturating_sub(THINKING_PREVIEW_LINES);
        let frame = self.spinner_frame.load(Ordering::Relaxed) % BRAILLE_SPINNER_FRAMES.len();
        let spinner = current_theme().fg(
            ColorToken::TextDim,
            &format!("{} ", BRAILLE_SPINNER_FRAMES[frame]),
        );
        let mut lines = vec![
            String::new(),
            format!(
                "{spinner}{}",
                current_theme().fg(ColorToken::TextDim, "thinking...")
            ),
        ];
        lines.extend(
            content_lines[start..]
                .iter()
                .map(|line| format!("{MESSAGE_INDENT}{line}")),
        );
        fit_lines(lines, width)
    }

    fn render_finalized(&self, width: usize, content_lines: &[String]) -> Vec<String> {
        let mut lines = vec![String::new()];
        for (index, line) in content_lines.iter().enumerate() {
            let prefix = if index == 0 && self.show_marker {
                current_theme().fg(ColorToken::TextDim, STATUS_BULLET)
            } else {
                MESSAGE_INDENT.to_owned()
            };
            lines.push(format!("{prefix}{line}"));
        }
        if self.expanded || content_lines.len() <= THINKING_PREVIEW_LINES {
            return fit_lines(lines, width);
        }

        let mut truncated = lines[..=THINKING_PREVIEW_LINES].to_vec();
        let remaining = content_lines.len() - THINKING_PREVIEW_LINES;
        let hint = format!("... ({remaining} more lines, ctrl+o to expand)");
        let indent_width = visible_width(MESSAGE_INDENT).min(width);
        let hint_width = width.saturating_sub(indent_width);
        truncated.push(format!(
            "{}{}",
            " ".repeat(indent_width),
            current_theme().dim(&truncate_to_width(&hint, hint_width, "…", false))
        ));
        fit_lines(truncated, width)
    }
}

impl Drop for ThinkingComponent {
    fn drop(&mut self) {
        self.stop_spinner();
    }
}

fn styled(text: &str) -> String {
    current_theme().italic_fg(ColorToken::TextDim, text)
}

fn advance_frame(frame: &AtomicUsize) {
    let current = frame.load(Ordering::Relaxed);
    frame.store(
        (current + 1) % BRAILLE_SPINNER_FRAMES.len(),
        Ordering::Relaxed,
    );
}

fn fit_lines(lines: Vec<String>, width: usize) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| truncate_to_width(&line, width, "…", false))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use regex::Regex;
    use std::sync::LazyLock;

    use super::*;

    fn strip(text: &str) -> String {
        static SGR: LazyLock<Regex> =
            LazyLock::new(|| Regex::new("\\x1b\\[[0-9;]*m").expect("valid SGR regex"));
        SGR.replace_all(text, "").into_owned()
    }

    fn long_thinking() -> String {
        (1..=7)
            .map(|number| format!("line{number}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn live_mode_shows_spinner_and_only_the_tail() {
        let mut component =
            ThinkingComponent::new(long_thinking(), true, ThinkingRenderMode::Live, None);
        let output = strip(&component.render(80).join("\n"));
        assert!(output.contains("⠋ thinking..."));
        assert!(!output.contains("  ⠋ thinking..."));
        assert!(!output.contains("line5"));
        assert!(output.contains("line6"));
        assert!(output.contains("line7"));
        assert!(!output.contains("ctrl+o to expand"));
    }

    #[test]
    fn spinner_requests_render_and_stops_on_finalize() {
        let (sender, receiver) = mpsc::channel();
        let callback = Arc::new(move || {
            let _ = sender.send(());
        });
        let mut component =
            ThinkingComponent::new("step", true, ThinkingRenderMode::Live, Some(callback));
        receiver
            .recv_timeout(Duration::from_millis(300))
            .expect("spinner should request a render");
        assert!(strip(&component.render(80).join("\n")).contains("⠙ thinking..."));

        component.finalize();
        while receiver.try_recv().is_ok() {}
        assert!(receiver.recv_timeout(Duration::from_millis(160)).is_err());
    }

    #[test]
    fn finalizes_into_collapsed_preview_and_toggles_expansion() {
        let mut component =
            ThinkingComponent::new(long_thinking(), true, ThinkingRenderMode::Live, None);
        component.finalize();
        let collapsed = strip(&component.render(80).join("\n"));
        assert!(collapsed.contains("line1"));
        assert!(collapsed.contains("line2"));
        assert!(!collapsed.contains("line3"));
        assert!(collapsed.contains("... (5 more lines, ctrl+o to expand)"));

        component.set_expanded(true);
        let expanded = strip(&component.render(80).join("\n"));
        assert!(expanded.contains("line7"));
        assert!(!expanded.contains("ctrl+o to expand"));

        component.set_expanded(false);
        assert!(strip(&component.render(80).join("\n")).contains("ctrl+o to expand"));
    }

    #[test]
    fn set_text_invalidate_and_footer_respect_width() {
        let mut component =
            ThinkingComponent::new(long_thinking(), false, ThinkingRenderMode::Finalized, None);
        for width in [37, 4, 1, 0] {
            assert!(
                component
                    .render(width)
                    .iter()
                    .all(|line| visible_width(line) <= width)
            );
        }
        component.set_text("replacement");
        component.invalidate();
        assert!(strip(&component.render(40).join("\n")).contains("replacement"));
    }
}
