use std::{
    path::Path,
    time::{Duration, Instant},
};

use serde_json::{Map, Value};

use crate::tui::{
    components::editor::{CustomEditor, EditorAction, InputMode},
    constant::kimi_tui::{
        CTRL_C_HINT, CTRL_D_HINT, DOUBLE_ESC_WINDOW_MS, EXIT_CONFIRM_WINDOW_MS,
        LLM_NOT_SET_MESSAGE, NO_ACTIVE_SESSION_MESSAGE,
    },
    types::{QueuedMessage, QueuedMessageMode, SteerInputItem},
    utils::{
        image_attachment_store::{ImageAttachmentOriginal, ImageAttachmentStore},
        image_placeholder::{ExtractionResult, extract_media_attachments},
    },
};
use crate::utils::process::external_editor::{
    ExternalEditorRuntime, edit_in_external_editor_with, resolve_editor_command,
};
use crate::utils::{
    clipboard::{ClipboardMedia, ClipboardMediaError, read_clipboard_media},
    image::{ImageMeta, parse_image_meta},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedClipboardImage {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub original: Option<ImageAttachmentOriginal>,
}

#[derive(Debug, Clone)]
pub struct ClipboardImageProcessingContext<'a> {
    pub max_edge: Option<u32>,
    pub session_directory: Option<&'a Path>,
}

#[async_trait::async_trait]
pub trait ClipboardMediaReader: Send + Sync {
    async fn read(&self) -> Result<Option<ClipboardMedia>, ClipboardMediaError>;
}

pub struct SystemClipboardMediaReader;

#[async_trait::async_trait]
impl ClipboardMediaReader for SystemClipboardMediaReader {
    async fn read(&self) -> Result<Option<ClipboardMedia>, ClipboardMediaError> {
        read_clipboard_media().await
    }
}

/// Agent-core image compression/persistence boundary used by the app layer.
#[async_trait::async_trait]
pub trait ClipboardImageProcessor: Send + Sync {
    async fn prepare(
        &self,
        bytes: Vec<u8>,
        meta: ImageMeta,
        context: ClipboardImageProcessingContext<'_>,
    ) -> PreparedClipboardImage;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamingPhase {
    Idle,
    Waiting,
    Thinking,
    Composing,
    Shell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveDialog {
    SessionPicker,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingExitKind {
    CtrlC,
    CtrlD,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorDispatch {
    None,
    PasteImageRequested,
    OpenExternalEditorRequested,
    AutocompleteRequested { force: bool },
}

pub trait EditorKeyboardHost {
    fn handle_user_input(&mut self, text: String);
    fn cancel_in_flight(&mut self) -> bool;
    fn cancel_btw_running(&mut self) -> bool;
    fn close_or_cancel_btw(&mut self) -> bool;
    fn is_compacting(&self) -> bool;
    fn streaming_phase(&self) -> StreamingPhase;
    fn cancel_current_compaction(&mut self);
    fn cancel_current_stream(&mut self);
    fn active_dialog(&self) -> Option<ActiveDialog>;
    fn hide_session_picker(&mut self);
    fn open_undo_selector(&mut self);
    fn has_session(&self) -> bool;
    fn model(&self) -> &str;
    fn show_error(&mut self, message: String);
    fn track(&mut self, event: &'static str, properties: Map<String, Value>);
    fn update_editor_border_highlight(&mut self, text: &str);
    fn request_stop(&mut self);
    fn plan_mode(&self) -> bool;
    fn handle_plan_toggle(&mut self, enabled: bool);
    fn handle_input_mode_change(&mut self, mode: InputMode);
    fn toggle_tool_output_expansion(&mut self);
    fn todo_has_overflow(&self) -> bool;
    fn toggle_todo_panel_expansion(&mut self);
    fn detach_current_foreground_task(&mut self);
    fn queued_messages(&self) -> &[QueuedMessage];
    fn replace_queued_messages(&mut self, messages: Vec<QueuedMessage>);
    fn validate_media_capabilities(&mut self, extraction: &ExtractionResult) -> bool;
    fn steer_message(&mut self, items: Vec<SteerInputItem>);
    fn update_queue_display(&mut self);
    fn request_render(&mut self, force: bool);
    fn recall_last_queued(&mut self) -> Option<QueuedMessage>;
    fn scroll_btw(&mut self, direction: BtwScrollDirection) -> bool;
    fn set_transient_hint(&mut self, hint: Option<&str>);
    fn external_editor_running(&self) -> bool;
    fn editor_command(&self) -> Option<&str>;
    fn set_external_editor_running(&mut self, running: bool);
    fn stop_ui(&mut self);
    fn start_ui(&mut self);
    fn focus_editor(&mut self);
    fn image_max_edge(&self) -> Option<u32>;
    fn session_directory(&self) -> Option<&Path>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtwScrollDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy)]
struct PendingExit {
    kind: PendingExitKind,
    expires_at: Instant,
}

/// Dispatches `CustomEditor` actions into TUI/session state transitions.
///
/// Original:
///   apps/kimi-code/src/tui/controllers/editor-keyboard.ts
///   EditorKeyboardController.install() and timer helpers
///
/// Rust adaptation:
///   `CustomEditor` emits typed actions instead of installing mutable JS
///   callbacks. The runtime feeds those actions here in order. Timeout expiry
///   is explicit so it can share the TUI event loop rather than detach tasks.
pub struct EditorKeyboardController {
    pending_exit: Option<PendingExit>,
    pending_undo_escape_until: Option<Instant>,
    last_editor_text: String,
}

impl Default for EditorKeyboardController {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorKeyboardController {
    pub fn new() -> Self {
        Self {
            pending_exit: None,
            pending_undo_escape_until: None,
            last_editor_text: String::new(),
        }
    }

    pub async fn handle_actions(
        &mut self,
        actions: impl IntoIterator<Item = EditorAction>,
        editor: &mut CustomEditor,
        image_store: &ImageAttachmentStore,
        host: &mut dyn EditorKeyboardHost,
        now: Instant,
    ) -> Vec<EditorDispatch> {
        self.expire(host, now);
        self.sync_editor_change(editor, host, true);
        let mut dispatches = Vec::new();
        for action in actions {
            let dispatch = self
                .handle_action(action, editor, image_store, host, now)
                .await;
            if dispatch != EditorDispatch::None {
                dispatches.push(dispatch);
            }
        }
        self.sync_editor_change(editor, host, false);
        dispatches
    }

    pub async fn handle_action(
        &mut self,
        action: EditorAction,
        editor: &mut CustomEditor,
        image_store: &ImageAttachmentStore,
        host: &mut dyn EditorKeyboardHost,
        now: Instant,
    ) -> EditorDispatch {
        match action {
            EditorAction::NonEscapeInput => self.clear_pending_undo_escape(),
            EditorAction::Submit(text) => host.handle_user_input(text),
            EditorAction::CtrlC => self.handle_ctrl_c(editor, host, now),
            EditorAction::CtrlD => self.handle_ctrl_d(host, now),
            EditorAction::Escape => self.handle_escape(host, now),
            EditorAction::ShiftTab => self.handle_shift_tab(host),
            EditorAction::InputModeChanged(mode) => host.handle_input_mode_change(mode),
            EditorAction::OpenExternalEditor => {
                track(host, "shortcut_editor");
                return EditorDispatch::OpenExternalEditorRequested;
            }
            EditorAction::ToggleToolExpand => {
                track(host, "shortcut_expand");
                host.toggle_tool_output_expansion();
            }
            EditorAction::ToggleTodoWithDefaultFallback => {
                if host.todo_has_overflow() {
                    self.clear_pending_exit(host);
                    track(host, "shortcut_todo_expand");
                    host.toggle_todo_panel_expansion();
                }
            }
            EditorAction::CtrlS => self.handle_steer(editor, image_store, host).await,
            EditorAction::CtrlBWithCursorLeftFallback => {
                if host.streaming_phase() == StreamingPhase::Idle || host.is_compacting() {
                    editor.apply_ctrl_b_fallback();
                } else {
                    track(host, "shortcut_background_task");
                    host.detach_current_foreground_task();
                }
            }
            EditorAction::UndoShortcut => track(host, "undo"),
            EditorAction::UpArrowEmptyWithHistoryFallback => {
                self.handle_up_arrow_empty(editor, host)
            }
            EditorAction::DownArrowEmptyWithHistoryFallback => {
                if !host.scroll_btw(BtwScrollDirection::Down) {
                    let changed = editor.apply_down_arrow_history_fallback();
                    if changed {
                        self.clear_pending_exit(host);
                    }
                }
            }
            EditorAction::PasteImage => return EditorDispatch::PasteImageRequested,
            EditorAction::RequestAutocomplete { force } => {
                return EditorDispatch::AutocompleteRequested { force };
            }
            EditorAction::AutocompleteCancelled => {}
        }
        EditorDispatch::None
    }

    pub fn expire(&mut self, host: &mut dyn EditorKeyboardHost, now: Instant) {
        if self
            .pending_exit
            .is_some_and(|pending| now >= pending.expires_at)
        {
            self.clear_pending_exit(host);
            host.request_render(false);
        }
        if self
            .pending_undo_escape_until
            .is_some_and(|expires_at| now >= expires_at)
        {
            self.pending_undo_escape_until = None;
        }
    }

    pub fn clear_pending_exit(&mut self, host: &mut dyn EditorKeyboardHost) {
        if self.pending_exit.take().is_some() {
            host.set_transient_hint(None);
        }
    }

    pub fn dispose(&mut self, host: &mut dyn EditorKeyboardHost) {
        self.clear_pending_exit(host);
        self.clear_pending_undo_escape();
    }

    // Original: EditorKeyboardController.openExternalEditor()
    pub async fn open_external_editor(
        &mut self,
        editor: &mut CustomEditor,
        host: &mut dyn EditorKeyboardHost,
        runtime: &dyn ExternalEditorRuntime,
    ) {
        if host.external_editor_running() {
            return;
        }
        let Some(command) = resolve_editor_command(host.editor_command()) else {
            host.show_error(
                "No editor configured. Set $VISUAL / $EDITOR, or run /editor <command>.".to_owned(),
            );
            return;
        };
        host.set_external_editor_running(true);
        let seed = editor.expanded_text();
        host.stop_ui();
        tokio::task::yield_now().await;
        match edit_in_external_editor_with(runtime, &seed, &command).await {
            Ok(Some(result)) => {
                let normalized = result.replace("\r\n", "\n");
                editor.set_text(normalized.strip_suffix('\n').unwrap_or(&normalized));
            }
            Ok(None) => {}
            Err(error) => host.show_error(format!("External editor failed: {error}")),
        }
        host.start_ui();
        host.focus_editor();
        host.request_render(true);
        host.set_external_editor_running(false);
    }

    // Original: EditorKeyboardController.handleClipboardImagePaste()
    pub async fn handle_clipboard_media_paste(
        &mut self,
        editor: &mut CustomEditor,
        image_store: &mut ImageAttachmentStore,
        host: &mut dyn EditorKeyboardHost,
        reader: &dyn ClipboardMediaReader,
        processor: &dyn ClipboardImageProcessor,
    ) -> bool {
        let media = match reader.read().await {
            Ok(Some(media)) => media,
            Ok(None) => return false,
            Err(error) => {
                host.show_error(error.to_string());
                return true;
            }
        };
        match media {
            ClipboardMedia::Video(video) => {
                let attachment = image_store.add_video(
                    video.mime_type,
                    video.source_path.to_string_lossy().into_owned(),
                    Some(&video.filename),
                );
                editor.insert_text_at_cursor(&format!("{} ", attachment.placeholder));
                host.request_render(false);
                host.track(
                    "shortcut_paste",
                    Map::from_iter([("kind".to_owned(), Value::String("video".to_owned()))]),
                );
                true
            }
            ClipboardMedia::Image(image) => {
                let Some(meta) = parse_image_meta(&image.bytes) else {
                    return false;
                };
                let session_directory = host.session_directory().map(Path::to_path_buf);
                let prepared = processor
                    .prepare(
                        image.bytes,
                        meta,
                        ClipboardImageProcessingContext {
                            max_edge: host.image_max_edge(),
                            session_directory: session_directory.as_deref(),
                        },
                    )
                    .await;
                let attachment = image_store.add_image(
                    prepared.bytes,
                    prepared.mime_type,
                    prepared.width,
                    prepared.height,
                    prepared.original,
                );
                editor.insert_text_at_cursor(&format!("{} ", attachment.placeholder));
                host.request_render(false);
                host.track(
                    "shortcut_paste",
                    Map::from_iter([("kind".to_owned(), Value::String("image".to_owned()))]),
                );
                true
            }
        }
    }

    fn handle_ctrl_c(
        &mut self,
        editor: &mut CustomEditor,
        host: &mut dyn EditorKeyboardHost,
        now: Instant,
    ) {
        if host.cancel_in_flight() {
            self.clear_pending_exit(host);
            return;
        }
        if host.cancel_btw_running() || host.close_or_cancel_btw() {
            self.clear_pending_exit(host);
            return;
        }
        if host.is_compacting() {
            self.clear_pending_exit(host);
            if !clear_editor_text_if_present(editor) {
                host.cancel_current_compaction();
            }
            return;
        }
        if host.streaming_phase() != StreamingPhase::Idle {
            self.clear_pending_exit(host);
            if !clear_editor_text_if_present(editor) {
                host.cancel_current_stream();
            }
            return;
        }
        if self
            .pending_exit
            .is_some_and(|pending| pending.kind == PendingExitKind::CtrlC)
        {
            self.clear_pending_exit(host);
            host.request_stop();
            return;
        }
        self.clear_pending_exit(host);
        clear_editor_text_if_present(editor);
        self.arm_pending_exit(PendingExitKind::CtrlC, CTRL_C_HINT, host, now);
    }

    fn handle_ctrl_d(&mut self, host: &mut dyn EditorKeyboardHost, now: Instant) {
        if self
            .pending_exit
            .is_some_and(|pending| pending.kind == PendingExitKind::CtrlD)
        {
            self.clear_pending_exit(host);
            host.request_stop();
        } else {
            self.arm_pending_exit(PendingExitKind::CtrlD, CTRL_D_HINT, host, now);
        }
    }

    fn handle_escape(&mut self, host: &mut dyn EditorKeyboardHost, now: Instant) {
        self.clear_pending_exit(host);
        if host.active_dialog() == Some(ActiveDialog::SessionPicker) {
            host.hide_session_picker();
            self.clear_pending_undo_escape();
            return;
        }
        if host.close_or_cancel_btw() {
            self.clear_pending_undo_escape();
            return;
        }
        if host.is_compacting() {
            host.cancel_current_compaction();
            self.clear_pending_undo_escape();
            return;
        }
        if host.streaming_phase() != StreamingPhase::Idle {
            host.cancel_current_stream();
            self.clear_pending_undo_escape();
            return;
        }
        if self.pending_undo_escape_until.is_some() {
            self.clear_pending_undo_escape();
            host.open_undo_selector();
        } else {
            self.pending_undo_escape_until =
                Some(now + Duration::from_millis(DOUBLE_ESC_WINDOW_MS));
        }
    }

    fn handle_shift_tab(&mut self, host: &mut dyn EditorKeyboardHost) {
        if !host.has_session() {
            host.show_error(NO_ACTIVE_SESSION_MESSAGE.to_owned());
            return;
        }
        let next = !host.plan_mode();
        host.track(
            "shortcut_plan_toggle",
            Map::from_iter([("enabled".to_owned(), Value::Bool(next))]),
        );
        host.track(
            "shortcut_mode_switch",
            Map::from_iter([(
                "to_mode".to_owned(),
                Value::String(if next { "plan" } else { "agent" }.to_owned()),
            )]),
        );
        host.handle_plan_toggle(next);
    }

    async fn handle_steer(
        &mut self,
        editor: &mut CustomEditor,
        image_store: &ImageAttachmentStore,
        host: &mut dyn EditorKeyboardHost,
    ) {
        if matches!(
            host.streaming_phase(),
            StreamingPhase::Idle | StreamingPhase::Shell
        ) || host.is_compacting()
        {
            return;
        }
        let text = editor.text().trim().to_owned();
        let editor_is_bash = editor.input_mode() == InputMode::Bash;
        let queued = host.queued_messages().to_vec();
        let mut items = queued
            .iter()
            .filter(|message| !message.is_bash())
            .filter_map(steer_item_from_queue)
            .collect::<Vec<_>>();

        let mut editor_extraction = None;
        if !editor_is_bash && !text.is_empty() {
            match extract_media_attachments(&text, image_store).await {
                Ok(extraction) => {
                    items.push(SteerInputItem {
                        text: text.clone(),
                        parts: extraction.has_media.then(|| extraction.parts.clone()),
                        image_attachment_ids: (!extraction.image_attachment_ids.is_empty())
                            .then(|| extraction.image_attachment_ids.clone()),
                    });
                    editor_extraction = Some(extraction);
                }
                Err(error) => {
                    host.show_error(format!("Failed to prepare media attachment: {error}"));
                    return;
                }
            }
        }
        if !items.is_empty() {
            if editor_extraction
                .as_ref()
                .is_some_and(|extraction| !host.validate_media_capabilities(extraction))
            {
                return;
            }
            host.replace_queued_messages(
                queued.into_iter().filter(QueuedMessage::is_bash).collect(),
            );
            if !editor_is_bash {
                self.clear_pending_exit(host);
                editor.set_text("");
            }
            if host.model().trim().is_empty() || !host.has_session() {
                host.show_error(LLM_NOT_SET_MESSAGE.to_owned());
            } else {
                host.steer_message(items);
            }
        }
        host.update_queue_display();
        host.request_render(false);
    }

    fn handle_up_arrow_empty(
        &mut self,
        editor: &mut CustomEditor,
        host: &mut dyn EditorKeyboardHost,
    ) {
        if host.scroll_btw(BtwScrollDirection::Up) {
            return;
        }
        if host.streaming_phase() == StreamingPhase::Idle && !host.is_compacting() {
            if editor.apply_up_arrow_history_fallback() {
                self.clear_pending_exit(host);
            }
            return;
        }
        let Some(recalled) = host.recall_last_queued() else {
            return;
        };
        self.clear_pending_exit(host);
        editor.set_text(&recalled.text);
        let mode = if recalled.mode == Some(QueuedMessageMode::Bash) {
            InputMode::Bash
        } else {
            InputMode::Prompt
        };
        if editor.set_input_mode(mode) {
            host.handle_input_mode_change(mode);
        }
        host.update_queue_display();
        host.request_render(false);
    }

    fn arm_pending_exit(
        &mut self,
        kind: PendingExitKind,
        hint: &'static str,
        host: &mut dyn EditorKeyboardHost,
        now: Instant,
    ) {
        self.clear_pending_exit(host);
        host.set_transient_hint(Some(hint));
        self.pending_exit = Some(PendingExit {
            kind,
            expires_at: now + Duration::from_millis(EXIT_CONFIRM_WINDOW_MS),
        });
        host.request_render(false);
    }

    fn clear_pending_undo_escape(&mut self) {
        self.pending_undo_escape_until = None;
    }

    fn sync_editor_change(
        &mut self,
        editor: &CustomEditor,
        host: &mut dyn EditorKeyboardHost,
        clear_exit: bool,
    ) {
        let text = editor.text();
        if text == self.last_editor_text {
            return;
        }
        if clear_exit {
            self.clear_pending_exit(host);
        }
        host.update_editor_border_highlight(&text);
        self.last_editor_text = text;
    }
}

fn clear_editor_text_if_present(editor: &mut CustomEditor) -> bool {
    if editor.text().is_empty() {
        false
    } else {
        editor.set_text("");
        true
    }
}

fn steer_item_from_queue(message: &QueuedMessage) -> Option<SteerInputItem> {
    let text = message.text.trim();
    (!text.is_empty()).then(|| SteerInputItem {
        text: text.to_owned(),
        parts: message.parts.clone(),
        image_attachment_ids: message.image_attachment_ids.clone(),
    })
}

fn track(host: &mut dyn EditorKeyboardHost, event: &'static str) {
    host.track(event, Map::new());
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[derive(Default)]
    struct Host {
        compacting: bool,
        phase: Option<StreamingPhase>,
        session: bool,
        model: String,
        plan: bool,
        dialog: Option<ActiveDialog>,
        queued: Vec<QueuedMessage>,
        recalled: Option<QueuedMessage>,
        events: Vec<String>,
        errors: Vec<String>,
        hints: Vec<Option<String>>,
        steered: Vec<Vec<SteerInputItem>>,
        validate: bool,
        todo_overflow: bool,
        cancel_in_flight: bool,
        btw_running: bool,
        btw_close: bool,
        external_editor_running: bool,
        editor_command: Option<String>,
        image_max_edge: Option<u32>,
        session_directory: Option<PathBuf>,
    }

    impl EditorKeyboardHost for Host {
        fn handle_user_input(&mut self, text: String) {
            self.events.push(format!("submit:{text}"));
        }
        fn cancel_in_flight(&mut self) -> bool {
            std::mem::take(&mut self.cancel_in_flight)
        }
        fn cancel_btw_running(&mut self) -> bool {
            std::mem::take(&mut self.btw_running)
        }
        fn close_or_cancel_btw(&mut self) -> bool {
            std::mem::take(&mut self.btw_close)
        }
        fn is_compacting(&self) -> bool {
            self.compacting
        }
        fn streaming_phase(&self) -> StreamingPhase {
            self.phase.unwrap_or(StreamingPhase::Idle)
        }
        fn cancel_current_compaction(&mut self) {
            self.events.push("cancel-compaction".to_owned());
        }
        fn cancel_current_stream(&mut self) {
            self.events.push("cancel-stream".to_owned());
        }
        fn active_dialog(&self) -> Option<ActiveDialog> {
            self.dialog
        }
        fn hide_session_picker(&mut self) {
            self.events.push("hide-picker".to_owned());
        }
        fn open_undo_selector(&mut self) {
            self.events.push("undo-selector".to_owned());
        }
        fn has_session(&self) -> bool {
            self.session
        }
        fn model(&self) -> &str {
            &self.model
        }
        fn show_error(&mut self, message: String) {
            self.errors.push(message);
        }
        fn track(&mut self, event: &'static str, _: Map<String, Value>) {
            self.events.push(event.to_owned());
        }
        fn update_editor_border_highlight(&mut self, text: &str) {
            self.events.push(format!("border:{text}"));
        }
        fn request_stop(&mut self) {
            self.events.push("stop".to_owned());
        }
        fn plan_mode(&self) -> bool {
            self.plan
        }
        fn handle_plan_toggle(&mut self, enabled: bool) {
            self.plan = enabled;
        }
        fn handle_input_mode_change(&mut self, mode: InputMode) {
            self.events.push(format!("mode:{mode:?}"));
        }
        fn toggle_tool_output_expansion(&mut self) {
            self.events.push("toggle-tools".to_owned());
        }
        fn todo_has_overflow(&self) -> bool {
            self.todo_overflow
        }
        fn toggle_todo_panel_expansion(&mut self) {
            self.events.push("toggle-todo".to_owned());
        }
        fn detach_current_foreground_task(&mut self) {
            self.events.push("detach".to_owned());
        }
        fn queued_messages(&self) -> &[QueuedMessage] {
            &self.queued
        }
        fn replace_queued_messages(&mut self, messages: Vec<QueuedMessage>) {
            self.queued = messages;
        }
        fn validate_media_capabilities(&mut self, _: &ExtractionResult) -> bool {
            self.validate
        }
        fn steer_message(&mut self, items: Vec<SteerInputItem>) {
            self.steered.push(items);
        }
        fn update_queue_display(&mut self) {
            self.events.push("queue".to_owned());
        }
        fn request_render(&mut self, force: bool) {
            self.events.push(format!("render:{force}"));
        }
        fn recall_last_queued(&mut self) -> Option<QueuedMessage> {
            self.recalled.take()
        }
        fn scroll_btw(&mut self, direction: BtwScrollDirection) -> bool {
            self.events.push(format!("scroll:{direction:?}"));
            false
        }
        fn set_transient_hint(&mut self, hint: Option<&str>) {
            self.hints.push(hint.map(str::to_owned));
        }
        fn external_editor_running(&self) -> bool {
            self.external_editor_running
        }
        fn editor_command(&self) -> Option<&str> {
            self.editor_command.as_deref()
        }
        fn set_external_editor_running(&mut self, running: bool) {
            self.external_editor_running = running;
            self.events.push(format!("editor-running:{running}"));
        }
        fn stop_ui(&mut self) {
            self.events.push("stop-ui".to_owned());
        }
        fn start_ui(&mut self) {
            self.events.push("start-ui".to_owned());
        }
        fn focus_editor(&mut self) {
            self.events.push("focus-editor".to_owned());
        }
        fn image_max_edge(&self) -> Option<u32> {
            self.image_max_edge
        }
        fn session_directory(&self) -> Option<&Path> {
            self.session_directory.as_deref()
        }
    }

    async fn action(
        controller: &mut EditorKeyboardController,
        action: EditorAction,
        editor: &mut CustomEditor,
        host: &mut Host,
        now: Instant,
    ) -> EditorDispatch {
        controller
            .handle_action(action, editor, &ImageAttachmentStore::new(), host, now)
            .await
    }

    #[tokio::test]
    async fn ctrl_c_clears_text_then_arms_and_second_press_stops() {
        let now = Instant::now();
        let mut controller = EditorKeyboardController::new();
        let mut editor = CustomEditor::new();
        let mut host = Host::default();
        editor.set_text("draft");
        action(
            &mut controller,
            EditorAction::CtrlC,
            &mut editor,
            &mut host,
            now,
        )
        .await;
        assert_eq!(editor.text(), "");
        assert_eq!(
            host.hints.last().and_then(|value| value.as_deref()),
            Some(CTRL_C_HINT)
        );
        action(
            &mut controller,
            EditorAction::CtrlC,
            &mut editor,
            &mut host,
            now,
        )
        .await;
        assert!(host.events.contains(&"stop".to_owned()));
        assert_eq!(host.hints.last(), Some(&None));
    }

    #[tokio::test]
    async fn batched_dispatch_keeps_new_exit_arm_but_typing_disarms_it() {
        let now = Instant::now();
        let mut controller = EditorKeyboardController::new();
        let mut editor = CustomEditor::new();
        let mut host = Host::default();
        controller
            .handle_actions(
                [EditorAction::CtrlC],
                &mut editor,
                &ImageAttachmentStore::new(),
                &mut host,
                now,
            )
            .await;
        assert_eq!(
            host.hints.last().and_then(|value| value.as_deref()),
            Some(CTRL_C_HINT)
        );

        let typed = editor.handle_input_event("x");
        controller
            .handle_actions(
                typed.actions,
                &mut editor,
                &ImageAttachmentStore::new(),
                &mut host,
                now,
            )
            .await;
        assert_eq!(host.hints.last(), Some(&None));
        controller
            .handle_actions(
                [EditorAction::CtrlC],
                &mut editor,
                &ImageAttachmentStore::new(),
                &mut host,
                now,
            )
            .await;
        assert!(!host.events.contains(&"stop".to_owned()));
    }

    #[tokio::test]
    async fn ctrl_c_priority_is_inflight_btw_compaction_then_stream() {
        let now = Instant::now();
        for (mut host, expected) in [
            (
                Host {
                    cancel_in_flight: true,
                    compacting: true,
                    ..Host::default()
                },
                None,
            ),
            (
                Host {
                    btw_running: true,
                    compacting: true,
                    ..Host::default()
                },
                None,
            ),
            (
                Host {
                    compacting: true,
                    ..Host::default()
                },
                Some("cancel-compaction"),
            ),
            (
                Host {
                    phase: Some(StreamingPhase::Thinking),
                    ..Host::default()
                },
                Some("cancel-stream"),
            ),
        ] {
            let mut controller = EditorKeyboardController::new();
            let mut editor = CustomEditor::new();
            action(
                &mut controller,
                EditorAction::CtrlC,
                &mut editor,
                &mut host,
                now,
            )
            .await;
            assert!(
                expected
                    .map(|value| host.events.contains(&value.to_owned()))
                    .unwrap_or(true)
            );
        }
    }

    #[tokio::test]
    async fn exit_hint_expires_and_double_escape_opens_undo_only_inside_window() {
        let now = Instant::now();
        let mut controller = EditorKeyboardController::new();
        let mut editor = CustomEditor::new();
        let mut host = Host::default();
        action(
            &mut controller,
            EditorAction::CtrlD,
            &mut editor,
            &mut host,
            now,
        )
        .await;
        controller.expire(
            &mut host,
            now + Duration::from_millis(EXIT_CONFIRM_WINDOW_MS + 1),
        );
        assert_eq!(host.hints.last(), Some(&None));
        action(
            &mut controller,
            EditorAction::Escape,
            &mut editor,
            &mut host,
            now,
        )
        .await;
        action(
            &mut controller,
            EditorAction::Escape,
            &mut editor,
            &mut host,
            now,
        )
        .await;
        assert!(host.events.contains(&"undo-selector".to_owned()));
    }

    #[tokio::test]
    async fn escape_dismisses_picker_btw_and_active_work_before_double_tap() {
        let now = Instant::now();
        for mut host in [
            Host {
                dialog: Some(ActiveDialog::SessionPicker),
                ..Host::default()
            },
            Host {
                btw_close: true,
                ..Host::default()
            },
            Host {
                compacting: true,
                ..Host::default()
            },
            Host {
                phase: Some(StreamingPhase::Waiting),
                ..Host::default()
            },
        ] {
            let mut controller = EditorKeyboardController::new();
            let mut editor = CustomEditor::new();
            action(
                &mut controller,
                EditorAction::Escape,
                &mut editor,
                &mut host,
                now,
            )
            .await;
            assert!(!host.events.contains(&"undo-selector".to_owned()));
        }
    }

    #[tokio::test]
    async fn shift_tab_requires_session_and_toggles_plan_with_telemetry() {
        let now = Instant::now();
        let mut controller = EditorKeyboardController::new();
        let mut editor = CustomEditor::new();
        let mut host = Host::default();
        action(
            &mut controller,
            EditorAction::ShiftTab,
            &mut editor,
            &mut host,
            now,
        )
        .await;
        assert_eq!(host.errors, [NO_ACTIVE_SESSION_MESSAGE]);
        host.session = true;
        action(
            &mut controller,
            EditorAction::ShiftTab,
            &mut editor,
            &mut host,
            now,
        )
        .await;
        assert!(host.plan);
        assert!(host.events.contains(&"shortcut_plan_toggle".to_owned()));
    }

    #[tokio::test]
    async fn ctrl_s_steers_prompt_queue_and_draft_but_keeps_bash_queue() {
        let now = Instant::now();
        let mut controller = EditorKeyboardController::new();
        let mut editor = CustomEditor::new();
        editor.set_text(" draft ");
        let mut host = Host {
            phase: Some(StreamingPhase::Composing),
            session: true,
            model: "kimi".to_owned(),
            validate: true,
            queued: vec![QueuedMessage::prompt(" first "), QueuedMessage::bash("ls")],
            ..Host::default()
        };
        action(
            &mut controller,
            EditorAction::CtrlS,
            &mut editor,
            &mut host,
            now,
        )
        .await;
        assert_eq!(host.queued, [QueuedMessage::bash("ls")]);
        assert_eq!(editor.text(), "");
        assert_eq!(
            host.steered[0]
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>(),
            ["first", "draft"]
        );
    }

    #[tokio::test]
    async fn ctrl_s_is_disabled_for_idle_shell_and_compaction() {
        let now = Instant::now();
        for mut host in [
            Host::default(),
            Host {
                phase: Some(StreamingPhase::Shell),
                ..Host::default()
            },
            Host {
                phase: Some(StreamingPhase::Thinking),
                compacting: true,
                ..Host::default()
            },
        ] {
            let mut controller = EditorKeyboardController::new();
            let mut editor = CustomEditor::new();
            editor.set_text("draft");
            action(
                &mut controller,
                EditorAction::CtrlS,
                &mut editor,
                &mut host,
                now,
            )
            .await;
            assert!(host.steered.is_empty());
            assert_eq!(editor.text(), "draft");
        }
    }

    #[tokio::test]
    async fn up_arrow_prefers_btw_then_history_or_running_queue_recall() {
        let now = Instant::now();
        let mut controller = EditorKeyboardController::new();
        let mut editor = CustomEditor::new();
        editor.add_to_history("history");
        let mut host = Host::default();
        action(
            &mut controller,
            EditorAction::UpArrowEmptyWithHistoryFallback,
            &mut editor,
            &mut host,
            now,
        )
        .await;
        assert_eq!(editor.text(), "history");
        editor.set_text("");
        host.phase = Some(StreamingPhase::Thinking);
        host.recalled = Some(QueuedMessage::bash("cargo test"));
        action(
            &mut controller,
            EditorAction::UpArrowEmptyWithHistoryFallback,
            &mut editor,
            &mut host,
            now,
        )
        .await;
        assert_eq!(editor.text(), "cargo test");
        assert_eq!(editor.input_mode(), InputMode::Bash);
    }

    #[tokio::test]
    async fn returns_runtime_dispatches_for_clipboard_editor_and_autocomplete() {
        let now = Instant::now();
        let mut controller = EditorKeyboardController::new();
        let mut editor = CustomEditor::new();
        let mut host = Host::default();
        assert_eq!(
            action(
                &mut controller,
                EditorAction::PasteImage,
                &mut editor,
                &mut host,
                now
            )
            .await,
            EditorDispatch::PasteImageRequested
        );
        assert_eq!(
            action(
                &mut controller,
                EditorAction::OpenExternalEditor,
                &mut editor,
                &mut host,
                now
            )
            .await,
            EditorDispatch::OpenExternalEditorRequested
        );
        assert_eq!(
            action(
                &mut controller,
                EditorAction::RequestAutocomplete { force: true },
                &mut editor,
                &mut host,
                now
            )
            .await,
            EditorDispatch::AutocompleteRequested { force: true }
        );
    }

    struct EditorRuntime {
        replacement: Option<String>,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl ExternalEditorRuntime for EditorRuntime {
        async fn run_shell(&self, command: &str) -> std::io::Result<i32> {
            if self.fail {
                return Err(std::io::Error::other("spawn failed"));
            }
            if let Some(replacement) = &self.replacement {
                let quote = if cfg!(windows) { '"' } else { '\'' };
                let end = command.rfind(quote).expect("closing quote");
                let start = command[..end].rfind(quote).expect("opening quote");
                tokio::fs::write(&command[start + 1..end], replacement).await?;
            }
            Ok(0)
        }
    }

    #[tokio::test]
    async fn external_editor_flow_normalizes_text_and_restores_ui() {
        let mut controller = EditorKeyboardController::new();
        let mut editor = CustomEditor::new();
        editor.set_text("seed");
        let mut host = Host {
            editor_command: Some("fake-editor --wait".to_owned()),
            ..Host::default()
        };
        controller
            .open_external_editor(
                &mut editor,
                &mut host,
                &EditorRuntime {
                    replacement: Some("edited\r\ntext\r\n".to_owned()),
                    fail: false,
                },
            )
            .await;
        assert_eq!(editor.text(), "edited\ntext");
        assert_eq!(
            host.events,
            [
                "editor-running:true",
                "stop-ui",
                "start-ui",
                "focus-editor",
                "render:true",
                "editor-running:false"
            ]
        );
    }

    #[tokio::test]
    async fn external_editor_reports_missing_command_and_spawn_failure() {
        let mut controller = EditorKeyboardController::new();
        let mut editor = CustomEditor::new();
        let mut host = Host::default();
        controller
            .open_external_editor(
                &mut editor,
                &mut host,
                &EditorRuntime {
                    replacement: None,
                    fail: false,
                },
            )
            .await;
        assert!(host.errors[0].starts_with("No editor configured."));

        host.editor_command = Some("fake".to_owned());
        controller
            .open_external_editor(
                &mut editor,
                &mut host,
                &EditorRuntime {
                    replacement: None,
                    fail: true,
                },
            )
            .await;
        assert_eq!(
            host.errors.last().map(String::as_str),
            Some("External editor failed: spawn failed")
        );
        assert!(!host.external_editor_running);
        assert_eq!(
            host.events.last().map(String::as_str),
            Some("editor-running:false")
        );
    }

    struct MediaReader(Result<Option<ClipboardMedia>, ClipboardMediaError>);

    #[async_trait::async_trait]
    impl ClipboardMediaReader for MediaReader {
        async fn read(&self) -> Result<Option<ClipboardMedia>, ClipboardMediaError> {
            self.0.clone()
        }
    }

    struct ImageProcessor;

    #[async_trait::async_trait]
    impl ClipboardImageProcessor for ImageProcessor {
        async fn prepare(
            &self,
            bytes: Vec<u8>,
            meta: ImageMeta,
            context: ClipboardImageProcessingContext<'_>,
        ) -> PreparedClipboardImage {
            assert_eq!(context.max_edge, Some(1_024));
            assert_eq!(context.session_directory, Some(Path::new("session")));
            PreparedClipboardImage {
                bytes,
                mime_type: meta.mime.as_str().to_owned(),
                width: meta.width,
                height: meta.height,
                original: None,
            }
        }
    }

    fn tiny_png() -> Vec<u8> {
        let mut bytes = vec![0; 24];
        bytes[..8].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        bytes[19] = 2;
        bytes[23] = 3;
        bytes
    }

    #[tokio::test]
    async fn clipboard_image_is_processed_stored_inserted_and_tracked() {
        let mut controller = EditorKeyboardController::new();
        let mut editor = CustomEditor::new();
        let mut store = ImageAttachmentStore::new();
        let mut host = Host {
            image_max_edge: Some(1_024),
            session_directory: Some(PathBuf::from("session")),
            ..Host::default()
        };
        let handled = controller
            .handle_clipboard_media_paste(
                &mut editor,
                &mut store,
                &mut host,
                &MediaReader(Ok(Some(ClipboardMedia::Image(
                    crate::utils::clipboard::ClipboardImage {
                        bytes: tiny_png(),
                        mime_type: "image/png".to_owned(),
                    },
                )))),
                &ImageProcessor,
            )
            .await;
        assert!(handled);
        assert_eq!(store.len(), 1);
        assert_eq!(editor.text(), "[image #1 (2×3)] ");
        assert!(host.events.contains(&"shortcut_paste".to_owned()));
    }

    #[tokio::test]
    async fn clipboard_video_is_stored_without_image_processing() {
        let mut controller = EditorKeyboardController::new();
        let mut editor = CustomEditor::new();
        let mut store = ImageAttachmentStore::new();
        let mut host = Host::default();
        let handled = controller
            .handle_clipboard_media_paste(
                &mut editor,
                &mut store,
                &mut host,
                &MediaReader(Ok(Some(ClipboardMedia::Video(
                    crate::utils::clipboard::ClipboardVideo {
                        mime_type: "video/mp4".to_owned(),
                        filename: "clip.mp4".to_owned(),
                        source_path: PathBuf::from("clip.mp4"),
                    },
                )))),
                &ImageProcessor,
            )
            .await;
        assert!(handled);
        assert_eq!(store.len(), 1);
        assert!(editor.text().starts_with("[video #1 clip.mp4]"));
    }

    #[tokio::test]
    async fn empty_or_invalid_clipboard_falls_back_to_text_paste() {
        let mut controller = EditorKeyboardController::new();
        let mut editor = CustomEditor::new();
        let mut store = ImageAttachmentStore::new();
        let mut host = Host::default();
        assert!(
            !controller
                .handle_clipboard_media_paste(
                    &mut editor,
                    &mut store,
                    &mut host,
                    &MediaReader(Ok(None)),
                    &ImageProcessor,
                )
                .await
        );
        assert!(
            !controller
                .handle_clipboard_media_paste(
                    &mut editor,
                    &mut store,
                    &mut host,
                    &MediaReader(Ok(Some(ClipboardMedia::Image(
                        crate::utils::clipboard::ClipboardImage {
                            bytes: b"not-image".to_vec(),
                            mime_type: "image/png".to_owned(),
                        },
                    )))),
                    &ImageProcessor,
                )
                .await
        );
    }
}
