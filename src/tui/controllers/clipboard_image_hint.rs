use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use tokio::task::JoinHandle;

use crate::tui::{
    constant::clipboard_image_hint::{FOCUS_DEBOUNCE, HINT_DISPLAY},
    utils::terminal_focus::{TERMINAL_FOCUS_IN, TERMINAL_FOCUS_OUT},
};

pub type DisposeClipboardInputListener = Box<dyn FnOnce() + Send>;
pub type ClipboardInputListener = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Debug)]
pub struct ClipboardProbeError(Box<dyn Error + Send + Sync>);

impl ClipboardProbeError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for ClipboardProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for ClipboardProbeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[async_trait]
pub trait ClipboardImageHintHost: Send + Sync + 'static {
    fn add_input_listener(&self, listener: ClipboardInputListener)
    -> DisposeClipboardInputListener;
    fn model_supports_image(&self) -> bool;
    async fn clipboard_has_image(&self) -> Result<bool, ClipboardProbeError>;
    fn transient_hint(&self) -> Option<String>;
    fn set_transient_hint(&self, hint: Option<String>);
    fn request_render(&self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardShortcutPlatform {
    Windows,
    Other,
}

impl ClipboardShortcutPlatform {
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

// Original: `getPasteImageShortcut()`.
pub const fn get_paste_image_shortcut(platform: ClipboardShortcutPlatform) -> &'static str {
    match platform {
        ClipboardShortcutPlatform::Windows => "Alt+V",
        ClipboardShortcutPlatform::Other => "Ctrl+V",
    }
}

#[derive(Debug)]
struct ClipboardHintState {
    check_generation: u64,
    focused: bool,
    initialized: bool,
    armed: bool,
    last_hint_text: Option<String>,
}

impl Default for ClipboardHintState {
    fn default() -> Self {
        Self {
            check_generation: 0,
            focused: true,
            initialized: false,
            armed: true,
            last_hint_text: None,
        }
    }
}

#[derive(Default)]
struct ClipboardHintTasks {
    baseline: Option<JoinHandle<()>>,
    debounce: Option<JoinHandle<()>>,
    clear_hint: Option<JoinHandle<()>>,
    dispose_input_listener: Option<DisposeClipboardInputListener>,
}

/// Focus-driven clipboard image hint lifecycle.
///
/// Original: `src/tui/controllers/clipboard-image-hint.ts`,
/// `ClipboardImageHintController`.
pub struct ClipboardImageHintController {
    host: Arc<dyn ClipboardImageHintHost>,
    state: Mutex<ClipboardHintState>,
    tasks: Mutex<ClipboardHintTasks>,
    shortcut_platform: ClipboardShortcutPlatform,
}

impl ClipboardImageHintController {
    pub fn new(host: Arc<dyn ClipboardImageHintHost>) -> Arc<Self> {
        Self::new_for_platform(host, ClipboardShortcutPlatform::current())
    }

    pub fn new_for_platform(
        host: Arc<dyn ClipboardImageHintHost>,
        shortcut_platform: ClipboardShortcutPlatform,
    ) -> Arc<Self> {
        Arc::new(Self {
            host,
            state: Mutex::new(ClipboardHintState::default()),
            tasks: Mutex::new(ClipboardHintTasks::default()),
            shortcut_platform,
        })
    }

    // Original: `start()`.
    pub fn start(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let listener = Arc::new(move |data: &str| {
            if let Some(controller) = weak.upgrade() {
                controller.handle_input(data);
            }
        });
        let dispose = self.host.add_input_listener(listener);
        let mut tasks = self.tasks.lock().expect("clipboard hint tasks");
        if let Some(previous) = tasks.dispose_input_listener.replace(dispose) {
            previous();
        }
        drop(tasks);
        self.establish_initial_baseline();
    }

    // Original: `stop()`.
    pub fn stop(&self) {
        let mut tasks = self.tasks.lock().expect("clipboard hint tasks");
        abort_task(&mut tasks.baseline);
        abort_task(&mut tasks.debounce);
        abort_task(&mut tasks.clear_hint);
        if let Some(dispose) = tasks.dispose_input_listener.take() {
            dispose();
        }
        drop(tasks);
        {
            let mut state = self.state.lock().expect("clipboard hint state");
            state.check_generation = state.check_generation.saturating_add(1);
        }
        self.clear_owned_hint();
        let mut state = self.state.lock().expect("clipboard hint state");
        state.initialized = false;
        state.armed = true;
    }

    // Original: `handleInput()`.
    pub fn handle_input(self: &Arc<Self>, data: &str) {
        match data {
            TERMINAL_FOCUS_IN => {
                self.state.lock().expect("clipboard hint state").focused = true;
                self.schedule_check();
            }
            TERMINAL_FOCUS_OUT => {
                self.state.lock().expect("clipboard hint state").focused = false;
                abort_task(&mut self.tasks.lock().expect("clipboard hint tasks").debounce);
            }
            _ => {}
        }
    }

    // Original: `scheduleCheck()`.
    fn schedule_check(self: &Arc<Self>) {
        let generation = {
            let mut state = self.state.lock().expect("clipboard hint state");
            state.check_generation = state.check_generation.saturating_add(1);
            state.check_generation
        };
        let mut tasks = self.tasks.lock().expect("clipboard hint tasks");
        abort_task(&mut tasks.debounce);
        let weak = Arc::downgrade(self);
        tasks.debounce = Some(tokio::spawn(async move {
            tokio::time::sleep(FOCUS_DEBOUNCE).await;
            if let Some(controller) = weak.upgrade() {
                controller.run_check(generation).await;
            }
        }));
    }

    // Original: `establishInitialBaseline()`.
    fn establish_initial_baseline(self: &Arc<Self>) {
        if !self.host.model_supports_image() {
            return;
        }
        let generation = {
            let mut state = self.state.lock().expect("clipboard hint state");
            state.check_generation = state.check_generation.saturating_add(1);
            state.check_generation
        };
        let weak = Arc::downgrade(self);
        let task = tokio::spawn(async move {
            let Some(controller) = weak.upgrade() else {
                return;
            };
            let Ok(has_image) = controller.host.clipboard_has_image().await else {
                return;
            };
            let mut state = controller.state.lock().expect("clipboard hint state");
            if state.check_generation != generation {
                return;
            }
            state.initialized = true;
            state.armed = !has_image;
        });
        let mut tasks = self.tasks.lock().expect("clipboard hint tasks");
        abort_task(&mut tasks.baseline);
        tasks.baseline = Some(task);
    }

    // Original: `runCheck()`.
    async fn run_check(self: Arc<Self>, generation: u64) {
        {
            let state = self.state.lock().expect("clipboard hint state");
            if !state.focused || !self.host.model_supports_image() {
                return;
            }
        }
        let Ok(has_image) = self.host.clipboard_has_image().await else {
            return;
        };
        let hint_text = {
            let mut state = self.state.lock().expect("clipboard hint state");
            if generation != state.check_generation || !state.focused {
                return;
            }
            if !state.initialized {
                state.initialized = true;
                state.armed = !has_image;
                return;
            }
            if !has_image {
                state.armed = true;
                return;
            }
            if !state.armed {
                return;
            }
            let hint = format!(
                "Image in clipboard · {} to paste",
                get_paste_image_shortcut(self.shortcut_platform)
            );
            state.last_hint_text = Some(hint.clone());
            state.armed = false;
            hint
        };

        let mut tasks = self.tasks.lock().expect("clipboard hint tasks");
        abort_task(&mut tasks.clear_hint);
        self.host.set_transient_hint(Some(hint_text));
        self.host.request_render();
        let weak = Arc::downgrade(&self);
        tasks.clear_hint = Some(tokio::spawn(async move {
            tokio::time::sleep(HINT_DISPLAY).await;
            if let Some(controller) = weak.upgrade() {
                controller.clear_owned_hint();
            }
        }));
    }

    // Original: `clearOwnedHint()`.
    fn clear_owned_hint(&self) {
        let last_hint = self
            .state
            .lock()
            .expect("clipboard hint state")
            .last_hint_text
            .take();
        if last_hint.is_some() && self.host.transient_hint() == last_hint {
            self.host.set_transient_hint(None);
            self.host.request_render();
        }
    }
}

fn abort_task(task: &mut Option<JoinHandle<()>>) {
    if let Some(task) = task.take() {
        task.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{
        MutexGuard,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use super::*;

    #[derive(Debug, Clone, Copy)]
    struct TestError;

    impl fmt::Display for TestError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("clipboard unavailable")
        }
    }

    impl Error for TestError {}

    struct HostMock {
        supports_image: AtomicBool,
        observations: Mutex<VecDeque<Result<bool, ClipboardProbeError>>>,
        hint: Mutex<Option<String>>,
        renders: AtomicUsize,
        listener: Arc<Mutex<Option<ClipboardInputListener>>>,
        disposals: Arc<AtomicUsize>,
    }

    impl HostMock {
        fn new(observations: impl IntoIterator<Item = bool>) -> Arc<Self> {
            Arc::new(Self {
                supports_image: AtomicBool::new(true),
                observations: Mutex::new(observations.into_iter().map(Ok).collect::<VecDeque<_>>()),
                hint: Mutex::new(None),
                renders: AtomicUsize::new(0),
                listener: Arc::new(Mutex::new(None)),
                disposals: Arc::new(AtomicUsize::new(0)),
            })
        }

        fn input(&self, data: &str) {
            self.listener
                .lock()
                .expect("listener")
                .as_ref()
                .expect("installed listener")(data);
        }

        fn hint(&self) -> MutexGuard<'_, Option<String>> {
            self.hint.lock().expect("hint")
        }
    }

    #[async_trait]
    impl ClipboardImageHintHost for HostMock {
        fn add_input_listener(
            &self,
            listener: ClipboardInputListener,
        ) -> DisposeClipboardInputListener {
            *self.listener.lock().expect("listener") = Some(listener);
            let listener_slot = Arc::clone(&self.listener);
            let disposals = Arc::clone(&self.disposals);
            Box::new(move || {
                *listener_slot.lock().expect("listener") = None;
                disposals.fetch_add(1, Ordering::Relaxed);
            })
        }

        fn model_supports_image(&self) -> bool {
            self.supports_image.load(Ordering::Relaxed)
        }

        async fn clipboard_has_image(&self) -> Result<bool, ClipboardProbeError> {
            self.observations
                .lock()
                .expect("observations")
                .pop_front()
                .unwrap_or_else(|| Err(ClipboardProbeError::new(TestError)))
        }

        fn transient_hint(&self) -> Option<String> {
            self.hint().clone()
        }

        fn set_transient_hint(&self, hint: Option<String>) {
            *self.hint() = hint;
        }

        fn request_render(&self) {
            self.renders.fetch_add(1, Ordering::Relaxed);
        }
    }

    async fn settle() {
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
    }

    #[test]
    fn shortcuts_match_platform_contract() {
        assert_eq!(
            get_paste_image_shortcut(ClipboardShortcutPlatform::Windows),
            "Alt+V"
        );
        assert_eq!(
            get_paste_image_shortcut(ClipboardShortcutPlatform::Other),
            "Ctrl+V"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn initial_image_establishes_quiet_baseline_until_clipboard_is_empty_then_new() {
        let host = HostMock::new([true, true, false, true]);
        let controller = ClipboardImageHintController::new_for_platform(
            Arc::clone(&host) as Arc<dyn ClipboardImageHintHost>,
            ClipboardShortcutPlatform::Windows,
        );
        controller.start();
        settle().await;
        assert_eq!(*host.hint(), None);

        host.input(TERMINAL_FOCUS_IN);
        settle().await;
        tokio::time::advance(FOCUS_DEBOUNCE).await;
        settle().await;
        assert_eq!(*host.hint(), None, "same baseline image remains disarmed");

        host.input(TERMINAL_FOCUS_IN);
        settle().await;
        tokio::time::advance(FOCUS_DEBOUNCE).await;
        settle().await;
        assert_eq!(*host.hint(), None, "empty clipboard rearms");

        host.input(TERMINAL_FOCUS_IN);
        settle().await;
        tokio::time::advance(FOCUS_DEBOUNCE).await;
        settle().await;
        assert_eq!(
            host.hint().as_deref(),
            Some("Image in clipboard · Alt+V to paste")
        );
        assert_eq!(host.renders.load(Ordering::Relaxed), 1);

        tokio::time::advance(HINT_DISPLAY).await;
        settle().await;
        assert_eq!(*host.hint(), None);
        assert_eq!(host.renders.load(Ordering::Relaxed), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn focus_out_cancels_debounce_and_unsupported_models_do_not_probe() {
        let host = HostMock::new([false, true]);
        host.supports_image.store(false, Ordering::Relaxed);
        let controller =
            ClipboardImageHintController::new(Arc::clone(&host) as Arc<dyn ClipboardImageHintHost>);
        controller.start();
        settle().await;
        assert_eq!(host.observations.lock().expect("observations").len(), 2);

        host.supports_image.store(true, Ordering::Relaxed);
        host.input(TERMINAL_FOCUS_IN);
        host.input(TERMINAL_FOCUS_OUT);
        settle().await;
        tokio::time::advance(FOCUS_DEBOUNCE).await;
        settle().await;
        assert_eq!(host.observations.lock().expect("observations").len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn stop_clears_only_the_hint_owned_by_this_controller() {
        let host = HostMock::new([false, true]);
        let controller =
            ClipboardImageHintController::new(Arc::clone(&host) as Arc<dyn ClipboardImageHintHost>);
        controller.start();
        settle().await;
        host.input(TERMINAL_FOCUS_IN);
        settle().await;
        tokio::time::advance(FOCUS_DEBOUNCE).await;
        settle().await;
        assert!(host.hint().is_some());
        controller.stop();
        assert_eq!(*host.hint(), None);
        assert_eq!(host.disposals.load(Ordering::Relaxed), 1);

        controller.start();
        settle().await;
        *host.hint() = Some("another controller".to_owned());
        controller.stop();
        assert_eq!(host.hint().as_deref(), Some("another controller"));
    }
}
