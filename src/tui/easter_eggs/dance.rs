use std::{
    sync::{
        Arc, LazyLock, RwLock,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::tui::{
    components::render::{truncate_to_width, visible_width},
    theme::current_theme,
};

pub const DANCE_FRAME_MS: u64 = 110;
pub const DANCE_FLOW_MS: u64 = 3_000;

const DARK_RAINBOW: &[&str] = &[
    "#4FA8FF", "#5BC0BE", "#4EC87E", "#E8A838", "#FFCB6B", "#C678B8", "#A274D9", "#7C8DFF",
];
const LIGHT_RAINBOW: &[&str] = &[
    "#1565C0", "#00838F", "#0E7A38", "#92660A", "#9A4A00", "#B91C1C", "#8A3A75", "#6B3A9A",
    "#354CB5",
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RainbowDanceView {
    pub colored: bool,
    pub phase: usize,
}

static NEXT_DANCE_ID: AtomicU64 = AtomicU64::new(1);
type SharedDanceView = Arc<RwLock<RainbowDanceView>>;
type CurrentDance = Option<(u64, SharedDanceView)>;
static CURRENT_DANCE: LazyLock<RwLock<CurrentDance>> = LazyLock::new(|| RwLock::new(None));

pub struct RainbowDance {
    id: u64,
    view: Arc<RwLock<RainbowDanceView>>,
    request_render: Arc<dyn Fn() + Send + Sync>,
    stop_sender: Option<Sender<()>>,
    worker: Option<JoinHandle<()>>,
    frame_interval: Duration,
    flow_duration: Duration,
}

impl RainbowDance {
    pub fn new(request_render: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::with_timings(
            request_render,
            Duration::from_millis(DANCE_FRAME_MS),
            Duration::from_millis(DANCE_FLOW_MS),
        )
    }

    fn with_timings(
        request_render: Arc<dyn Fn() + Send + Sync>,
        frame_interval: Duration,
        flow_duration: Duration,
    ) -> Self {
        let id = NEXT_DANCE_ID.fetch_add(1, Ordering::Relaxed);
        let view = Arc::new(RwLock::new(RainbowDanceView::default()));
        set_current_dance(Some((id, Arc::clone(&view))));
        Self {
            id,
            view,
            request_render,
            stop_sender: None,
            worker: None,
            frame_interval,
            flow_duration,
        }
    }

    // Original: RainbowDance.start()
    pub fn start(&mut self, hold: bool) {
        self.stop_worker();
        update_view(&self.view, |view| view.colored = true);
        (self.request_render)();
        let view = Arc::clone(&self.view);
        let request_render = Arc::clone(&self.request_render);
        let frame_interval = self.frame_interval;
        let flow_duration = self.flow_duration;
        let (sender, receiver) = mpsc::channel();
        self.stop_sender = Some(sender);
        self.worker = Some(thread::spawn(move || {
            let started = Instant::now();
            loop {
                let remaining = flow_duration.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    if !hold {
                        update_view(&view, |view| {
                            view.colored = false;
                            view.phase = 0;
                        });
                    }
                    request_render();
                    break;
                }
                match receiver.recv_timeout(frame_interval.min(remaining)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if started.elapsed() >= flow_duration {
                            continue;
                        }
                        update_view(&view, |view| view.phase = view.phase.wrapping_add(1));
                        request_render();
                    }
                }
            }
        }));
    }

    // Original: RainbowDance.stop()
    pub fn stop(&mut self) {
        self.stop_worker();
        update_view(&self.view, |view| *view = RainbowDanceView::default());
        (self.request_render)();
    }

    // Original: RainbowDance.dispose()
    pub fn dispose(&mut self) {
        self.stop_worker();
    }

    pub fn view(&self) -> RainbowDanceView {
        read_view(&self.view)
    }

    fn stop_worker(&mut self) {
        if let Some(sender) = self.stop_sender.take() {
            let _ = sender.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for RainbowDance {
    fn drop(&mut self) {
        self.dispose();
        let should_clear = match CURRENT_DANCE.read() {
            Ok(current) => current.as_ref().is_some_and(|(id, _)| *id == self.id),
            Err(poisoned) => poisoned
                .into_inner()
                .as_ref()
                .is_some_and(|(id, _)| *id == self.id),
        };
        if should_clear {
            set_current_dance(None);
        }
    }
}

pub fn get_rainbow_dance_view() -> Option<RainbowDanceView> {
    match CURRENT_DANCE.read() {
        Ok(current) => current.as_ref().map(|(_, view)| read_view(view)),
        Err(poisoned) => poisoned
            .into_inner()
            .as_ref()
            .map(|(_, view)| read_view(view)),
    }
}

pub fn is_rainbow_dancing() -> bool {
    get_rainbow_dance_view().is_some_and(|view| view.colored)
}

// Original: dance.ts rainbowText()
pub fn rainbow_text(text: &str, colors: &[&str], offset: usize, bold: bool) -> String {
    if colors.is_empty() {
        return text.to_owned();
    }
    let theme = current_theme();
    let mut color_index = offset;
    text.chars()
        .map(|character| {
            if character == ' ' {
                return " ".to_owned();
            }
            let color = colors[color_index % colors.len()];
            color_index += 1;
            if bold {
                theme.bold_fg_hex(color, &character.to_string())
            } else {
                theme.fg_hex(color, &character.to_string())
            }
        })
        .collect()
}

pub fn render_dance_welcome_header(
    logo: [&str; 2],
    text_width: usize,
    right_row_1: &str,
) -> Vec<String> {
    let phase = get_rainbow_dance_view().map_or(0, |view| view.phase);
    let palette = dance_palette();
    let logo_width = logo.iter().map(|row| visible_width(row)).max().unwrap_or(0);
    let row_0 = truncate_to_width(
        &rainbow_text("Welcome to Kimi Code!", palette, phase + 2, true),
        text_width,
        "…",
        false,
    );
    vec![
        format!(
            "{}  {row_0}",
            rainbow_text(&format!("{:<logo_width$}", logo[0]), palette, phase, false)
        ),
        format!(
            "{}  {right_row_1}",
            rainbow_text(
                &format!("{:<logo_width$}", logo[1]),
                palette,
                phase + 3,
                false
            )
        ),
    ]
}

pub fn render_dance_footer_model(model_label: &str) -> String {
    let phase = get_rainbow_dance_view().map_or(0, |view| view.phase);
    rainbow_text(model_label, dance_palette(), phase, false)
}

fn dance_palette() -> &'static [&'static str] {
    if current_theme().palette().text == "#1A1A1A" {
        LIGHT_RAINBOW
    } else {
        DARK_RAINBOW
    }
}

fn set_current_dance(value: CurrentDance) {
    match CURRENT_DANCE.write() {
        Ok(mut current) => *current = value,
        Err(poisoned) => *poisoned.into_inner() = value,
    }
}

fn update_view(view: &RwLock<RainbowDanceView>, update: impl FnOnce(&mut RainbowDanceView)) {
    match view.write() {
        Ok(mut view) => update(&mut view),
        Err(poisoned) => update(&mut poisoned.into_inner()),
    }
}

fn read_view(view: &RwLock<RainbowDanceView>) -> RainbowDanceView {
    match view.read() {
        Ok(view) => *view,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    static TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    #[test]
    fn rainbow_skips_spaces_and_cycles_colors() {
        let output = rainbow_text("a b", &["#ff0000", "#00ff00"], 0, false);
        assert!(output.contains("38;2;255;0;0"));
        assert!(output.contains("38;2;0;255;0"));
        assert!(output.contains("m \u{1b}"));
    }

    #[test]
    fn start_stop_and_drop_publish_the_global_view() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let renders = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&renders);
        let mut dance = RainbowDance::with_timings(
            Arc::new(move || {
                count.fetch_add(1, Ordering::Relaxed);
            }),
            Duration::from_millis(5),
            Duration::from_millis(25),
        );
        assert!(!is_rainbow_dancing());
        dance.start(false);
        assert!(is_rainbow_dancing());
        dance.stop();
        assert_eq!(dance.view(), RainbowDanceView::default());
        assert!(renders.load(Ordering::Relaxed) >= 2);
        drop(dance);
        assert!(get_rainbow_dance_view().is_none());
    }

    #[test]
    fn welcome_and_footer_helpers_use_current_phase() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut dance = RainbowDance::new(Arc::new(|| {}));
        dance.start(true);
        let header = render_dance_welcome_header(["▐█▛█▛█▌", "▐█████▌"], 30, "help");
        assert_eq!(header.len(), 2);
        let plain = regex::Regex::new(r"\x1b\[[0-9;]*m")
            .expect("ANSI regex")
            .replace_all(&header[0], "")
            .into_owned();
        assert!(plain.contains("Welcome"));
        assert!(render_dance_footer_model("model").contains("38;2;"));
        dance.stop();
    }
}
