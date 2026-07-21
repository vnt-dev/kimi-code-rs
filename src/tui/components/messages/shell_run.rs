use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::task::JoinHandle;

use crate::tui::{
    components::{Component, ComponentRole, Text},
    theme::{ColorToken, current_theme},
    utils::shell_output::{format_bash_output_for_display, sanitize_shell_output},
};

const RUNNING_TAIL_LINES: usize = 5;
const TIMER_INTERVAL: Duration = Duration::from_secs(1);
const MAX_COMBINED_UTF16_UNITS: usize = 256 * 1024;
const KEEP_COMBINED_UTF16_UNITS: usize = 64 * 1024;

type RenderCallback = dyn FnMut() + Send;

pub struct ShellRunComponent {
    text_component: Text,
    combined: String,
    running: bool,
    backgrounded: bool,
    disposed: bool,
    final_stdout: String,
    final_stderr: String,
    final_is_error: Option<bool>,
    started_at: Instant,
    request_render: Arc<Mutex<Box<RenderCallback>>>,
    timer_running: Arc<AtomicBool>,
    timer: Option<JoinHandle<()>>,
}

impl ShellRunComponent {
    pub fn new(request_render: impl FnMut() + Send + 'static) -> Self {
        let request_render: Arc<Mutex<Box<RenderCallback>>> =
            Arc::new(Mutex::new(Box::new(request_render)));
        let timer_running = Arc::new(AtomicBool::new(true));
        let timer = start_timer(Arc::clone(&request_render), Arc::clone(&timer_running));
        let mut component = Self {
            text_component: Text::new(String::new(), 0, 0),
            combined: String::new(),
            running: true,
            backgrounded: false,
            disposed: false,
            final_stdout: String::new(),
            final_stderr: String::new(),
            final_is_error: None,
            started_at: Instant::now(),
            request_render,
            timer_running,
            timer,
        };
        component.refresh_text();
        component
    }

    /// Original: shell-run.ts ShellRunComponent.append()
    pub fn append(&mut self, text: &str) {
        if self.disposed || !self.running || text.is_empty() {
            return;
        }
        self.combined.push_str(text);
        if self.combined.encode_utf16().count() > MAX_COMBINED_UTF16_UNITS {
            self.combined = utf16_tail(&self.combined, KEEP_COMBINED_UTF16_UNITS);
        }
        self.flush();
    }

    /// Original: shell-run.ts ShellRunComponent.finish()
    pub fn finish(
        &mut self,
        stdout: impl Into<String>,
        stderr: impl Into<String>,
        is_error: Option<bool>,
    ) {
        if self.disposed || !self.running {
            return;
        }
        self.running = false;
        self.final_stdout = stdout.into();
        self.final_stderr = stderr.into();
        self.final_is_error = is_error;
        self.clear_timer();
        self.flush();
    }

    /// Original: shell-run.ts ShellRunComponent.finishBackgrounded()
    pub fn finish_backgrounded(&mut self) {
        if self.disposed || !self.running {
            return;
        }
        self.running = false;
        self.backgrounded = true;
        self.clear_timer();
        self.flush();
    }

    pub fn dispose(&mut self) {
        self.disposed = true;
        self.clear_timer();
    }

    fn clear_timer(&mut self) {
        self.timer_running.store(false, Ordering::Release);
        if let Some(timer) = self.timer.take() {
            timer.abort();
        }
    }

    fn flush(&mut self) {
        if self.disposed {
            return;
        }
        self.refresh_text();
        invoke_render_callback(&self.request_render);
    }

    fn refresh_text(&mut self) {
        self.text_component.set_text(self.render_text());
    }

    /// Original: shell-run.ts ShellRunComponent.renderText()
    fn render_text(&self) -> String {
        if self.backgrounded {
            return format!(
                "  {}",
                current_theme().fg(ColorToken::TextDim, "Moved to background.")
            );
        }
        if !self.running {
            return format_bash_output_for_display(
                &self.final_stdout,
                &self.final_stderr,
                self.final_is_error,
            )
            .split('\n')
            .map(|line| format!("  {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        }

        let elapsed = self.started_at.elapsed().as_secs();
        let trimmed = sanitize_shell_output(&self.combined).trim_end().to_owned();
        let (body, extra) = if trimmed.is_empty() {
            (
                format!("  {}", current_theme().fg(ColorToken::TextDim, "Running…")),
                0,
            )
        } else {
            let lines = trimmed.split('\n').collect::<Vec<_>>();
            let extra = lines.len().saturating_sub(RUNNING_TAIL_LINES);
            let body = lines
                .iter()
                .skip(extra)
                .map(|line| format!("  {}", current_theme().fg(ColorToken::TextDim, line)))
                .collect::<Vec<_>>()
                .join("\n");
            (body, extra)
        };
        let timing = if extra > 0 {
            format!("+{extra} lines ({elapsed}s)")
        } else {
            format!("({elapsed}s)")
        };
        format!(
            "{body}\n  {}\n  {}",
            current_theme().fg(ColorToken::TextDim, &timing),
            current_theme().fg(ColorToken::TextDim, "(ctrl+b to run in background)")
        )
    }
}

impl Component for ShellRunComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        if !self.disposed {
            self.refresh_text();
        }
        self.text_component.render(width.max(1))
    }

    fn invalidate(&mut self) {
        if !self.disposed {
            self.refresh_text();
        }
        self.text_component.invalidate();
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Drop for ShellRunComponent {
    fn drop(&mut self) {
        self.clear_timer();
    }
}

fn start_timer(
    request_render: Arc<Mutex<Box<RenderCallback>>>,
    running: Arc<AtomicBool>,
) -> Option<JoinHandle<()>> {
    let handle = tokio::runtime::Handle::try_current().ok()?;
    Some(handle.spawn(async move {
        let start = tokio::time::Instant::now() + TIMER_INTERVAL;
        let mut interval = tokio::time::interval_at(start, TIMER_INTERVAL);
        while running.load(Ordering::Acquire) {
            interval.tick().await;
            if !running.load(Ordering::Acquire) {
                break;
            }
            invoke_render_callback(&request_render);
        }
    }))
}

fn invoke_render_callback(callback: &Arc<Mutex<Box<RenderCallback>>>) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Ok(mut callback) = callback.lock() {
            callback();
        }
    }));
}

fn utf16_tail(text: &str, units_to_keep: usize) -> String {
    let units = text.encode_utf16().collect::<Vec<_>>();
    let start = units.len().saturating_sub(units_to_keep);
    String::from_utf16_lossy(&units[start..])
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    fn strip_sgr(text: &str) -> String {
        let mut output = String::new();
        let mut escape = false;
        for character in text.chars() {
            if character == '\u{1b}' {
                escape = true;
            } else if escape && character == 'm' {
                escape = false;
            } else if !escape {
                output.push(character);
            }
        }
        output
    }

    #[tokio::test]
    async fn caps_huge_running_buffer_and_shows_tail() {
        let mut component = ShellRunComponent::new(|| {});
        let chunk = "x".repeat(50_000);
        for _ in 0..20 {
            component.append(&chunk);
        }
        assert!(component.combined.encode_utf16().count() <= MAX_COMBINED_UTF16_UNITS);
        assert!(!component.render(100).is_empty());
        component.dispose();
    }

    #[tokio::test]
    async fn finish_uses_final_view_and_ignores_later_appends() {
        let mut component = ShellRunComponent::new(|| {});
        component.finish("final output", "", Some(false));
        component.append("should be ignored");
        let rendered = strip_sgr(&component.render(100).join("\n"));
        assert!(rendered.contains("final output"));
        assert!(!rendered.contains("should be ignored"));
    }

    #[tokio::test]
    async fn finish_backgrounded_renders_hint() {
        let mut component = ShellRunComponent::new(|| {});
        component.finish_backgrounded();
        assert!(strip_sgr(&component.render(100).join("\n")).contains("Moved to background."));
    }

    #[tokio::test]
    async fn operations_after_dispose_are_noops() {
        let mut component = ShellRunComponent::new(|| {});
        component.dispose();
        component.append("late");
        component.finish("late", "", Some(false));
        component.finish_backgrounded();
        assert!(!component.render(100).is_empty());
        assert!(!component.combined.contains("late"));
    }

    #[tokio::test]
    async fn catches_panicking_render_callback() {
        let mut component = ShellRunComponent::new(|| panic!("render failed"));
        component.append("output");
        assert!(strip_sgr(&component.render(100).join("\n")).contains("output"));
        component.dispose();
    }

    #[tokio::test(start_paused = true)]
    async fn timer_requests_render_once_per_second_until_finished() {
        let calls = Arc::new(AtomicUsize::new(0));
        let captured = Arc::clone(&calls);
        let mut component = ShellRunComponent::new(move || {
            captured.fetch_add(1, Ordering::Relaxed);
        });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(3)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::Relaxed), 3);
        component.finish("done", "", None);
        let after_finish = calls.load(Ordering::Relaxed);
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(calls.load(Ordering::Relaxed), after_finish);
    }
}
