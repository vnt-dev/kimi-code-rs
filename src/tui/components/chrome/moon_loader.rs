use std::{
    any::Any,
    sync::{
        Arc, Mutex,
        mpsc::{self, Sender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::tui::{
    components::{
        Component, ComponentRole,
        render::{truncate_to_width, visible_width},
    },
    theme::{ColorToken, current_theme},
};

const BRAILLE_SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BRAILLE_SPINNER_INTERVAL_MS: u64 = 80;
const MOON_SPINNER_FRAMES: &[&str] = &["🌑", "🌒", "🌓", "🌔", "🌕", "🌖", "🌗", "🌘"];
const MOON_SPINNER_INTERVAL_MS: u64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpinnerStyle {
    Moon,
    Braille,
}

type ColorFn = dyn Fn(&str) -> String + Send + Sync;

struct LoaderState {
    current_frame: usize,
    frames: &'static [&'static str],
    color_fn: Option<Arc<ColorFn>>,
    label: String,
    tip: String,
    available_width: usize,
}

impl LoaderState {
    fn inline_text(&self) -> String {
        let frame = self.frames[self.current_frame];
        let colored_frame = self
            .color_fn
            .as_ref()
            .map_or_else(|| frame.to_owned(), |color| color(frame));
        if self.label.is_empty() {
            colored_frame
        } else {
            format!("{colored_frame} {}", self.label)
        }
    }

    fn display_text(&self) -> String {
        let base = self.inline_text();
        if self.tip.is_empty() {
            return base;
        }
        let with_tip = format!(
            "{base}{}",
            current_theme().fg(ColorToken::TextDim, &self.tip)
        );
        if self.available_width == 0 || visible_width(&with_tip) <= self.available_width {
            with_tip
        } else {
            base
        }
    }
}

/// Animated moon/braille loader used by the activity pane and AgentSwarm.
///
/// Original: `src/tui/components/chrome/moon-loader.ts`, `MoonLoader`.
pub struct MoonLoader {
    state: Arc<Mutex<LoaderState>>,
    request_render: Arc<dyn Fn() + Send + Sync>,
    interval: Duration,
    stop_sender: Option<Sender<()>>,
    timer: Option<JoinHandle<()>>,
}

impl MoonLoader {
    pub fn new(
        request_render: Arc<dyn Fn() + Send + Sync>,
        style: SpinnerStyle,
        color_fn: Option<Arc<ColorFn>>,
        label: impl Into<String>,
    ) -> Self {
        let (frames, interval_ms) = match style {
            SpinnerStyle::Moon => (MOON_SPINNER_FRAMES, MOON_SPINNER_INTERVAL_MS),
            SpinnerStyle::Braille => (BRAILLE_SPINNER_FRAMES, BRAILLE_SPINNER_INTERVAL_MS),
        };
        let mut loader = Self {
            state: Arc::new(Mutex::new(LoaderState {
                current_frame: 0,
                frames,
                color_fn,
                label: label.into(),
                tip: String::new(),
                available_width: 0,
            })),
            request_render,
            interval: Duration::from_millis(interval_ms),
            stop_sender: None,
            timer: None,
        };
        loader.start();
        loader
    }

    // Original: MoonLoader.start()
    pub fn start(&mut self) {
        if self.timer.is_some() {
            return;
        }
        (self.request_render)();
        let (sender, receiver) = mpsc::channel();
        let state = Arc::clone(&self.state);
        let request_render = Arc::clone(&self.request_render);
        let interval = self.interval;
        self.stop_sender = Some(sender);
        self.timer = Some(thread::spawn(move || {
            while receiver.recv_timeout(interval).is_err() {
                with_state(&state, |state| {
                    state.current_frame = (state.current_frame + 1) % state.frames.len();
                });
                request_render();
            }
        }));
    }

    // Original: MoonLoader.stop()/dispose()
    pub fn stop(&mut self) {
        if let Some(sender) = self.stop_sender.take() {
            let _ = sender.send(());
        }
        if let Some(timer) = self.timer.take() {
            let _ = timer.join();
        }
    }

    pub fn dispose(&mut self) {
        self.stop();
    }

    pub fn set_label(&mut self, label: impl Into<String>) {
        with_state(&self.state, |state| state.label = label.into());
        (self.request_render)();
    }

    pub fn set_color_fn(&mut self, color_fn: Arc<ColorFn>) {
        with_state(&self.state, |state| state.color_fn = Some(color_fn));
        (self.request_render)();
    }

    pub fn set_tip(&mut self, tip: impl Into<String>) {
        with_state(&self.state, |state| state.tip = tip.into());
        (self.request_render)();
    }

    pub fn set_available_width(&mut self, width: usize) {
        let changed = with_state_result(&self.state, |state| {
            if state.available_width == width {
                false
            } else {
                state.available_width = width;
                true
            }
        });
        if changed {
            (self.request_render)();
        }
    }

    pub fn render_inline(&self) -> String {
        with_state_result(&self.state, |state| state.inline_text())
    }

    fn display_text(&self) -> String {
        with_state_result(&self.state, |state| state.display_text())
    }
}

impl Drop for MoonLoader {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Component for MoonLoader {
    fn render(&mut self, width: usize) -> Vec<String> {
        let content_width = width.saturating_sub(2).max(1);
        let text = truncate_to_width(&self.display_text(), content_width, "…", false);
        vec![truncate_to_width(&format!(" {text} "), width, "", false)]
    }

    fn invalidate(&mut self) {}

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn with_state(state: &Mutex<LoaderState>, update: impl FnOnce(&mut LoaderState)) {
    match state.lock() {
        Ok(mut state) => update(&mut state),
        Err(poisoned) => update(&mut poisoned.into_inner()),
    }
}

fn with_state_result<T>(state: &Mutex<LoaderState>, read: impl FnOnce(&mut LoaderState) -> T) -> T {
    match state.lock() {
        Ok(mut state) => read(&mut state),
        Err(poisoned) => read(&mut poisoned.into_inner()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn loader() -> MoonLoader {
        MoonLoader::new(Arc::new(|| {}), SpinnerStyle::Moon, None, "")
    }

    #[test]
    fn inline_text_excludes_tip_while_standalone_row_includes_it() {
        let mut loader = loader();
        loader.set_tip(" · Tip: ctrl+s: steer mid-turn");
        loader.set_available_width(80);
        assert!(!loader.render_inline().contains("Tip"));
        assert!(loader.render(80).join("\n").contains("Tip: ctrl+s"));
        loader.stop();
    }

    #[test]
    fn hides_tip_when_available_width_is_too_small() {
        let mut loader = loader();
        loader.set_label("Working");
        loader.set_tip(" · Tip: a very long hint");
        loader.set_available_width(10);
        assert!(!loader.render(80).join("\n").contains("Tip"));
        assert!(loader.render_inline().contains("Working"));
        loader.stop();
    }

    #[test]
    fn start_requests_initial_frame_and_stop_is_idempotent() {
        let renders = Arc::new(AtomicUsize::new(0));
        let callback_count = Arc::clone(&renders);
        let mut loader = MoonLoader::new(
            Arc::new(move || {
                callback_count.fetch_add(1, Ordering::Relaxed);
            }),
            SpinnerStyle::Braille,
            None,
            "",
        );
        loader.stop();
        loader.stop();
        assert!(renders.load(Ordering::Relaxed) >= 1);
    }
}
