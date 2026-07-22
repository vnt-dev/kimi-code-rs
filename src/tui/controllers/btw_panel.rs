use std::{
    collections::HashMap,
    error::Error,
    sync::{Arc, Mutex, MutexGuard},
};

use async_trait::async_trait;

use crate::{
    sdk::{
        events::{AgentEvent, Event, KimiErrorPayload, TurnEndReason},
        types::HookResultEvent,
    },
    tui::{
        components::{
            MarkdownOptions,
            panes::btw_panel::{BtwPanelComponent, BtwPanelOptions, BtwScrollDirection},
        },
        constant::kimi_tui::NO_ACTIVE_SESSION_MESSAGE,
        utils::{
            event_payload::format_error_message, hook_result_format::format_hook_result_plain,
        },
    },
};

const BTW_BUSY_NOTICE: &str = "Wait for /btw to finish before sending another question.";

pub type BtwRuntimeError = Box<dyn Error + Send + Sync>;
pub type SharedBtwPanel = Arc<Mutex<BtwPanelComponent>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BtwAsyncAction {
    Prompt { agent_id: String, prompt: String },
    Cancel { agent_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtwCloseResult {
    pub closed: bool,
    pub action: Option<BtwAsyncAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtwSendResult {
    pub handled: bool,
    pub action: Option<BtwAsyncAction>,
}

pub trait BtwPanelHost {
    fn mount_btw_panel(&mut self, panel: SharedBtwPanel);
    fn unmount_btw_panel(&mut self, force_render: bool);
    fn set_editor_text(&mut self, text: &str);
    fn focus_editor(&mut self);
    fn request_render(&mut self, force: bool);
    fn show_error(&mut self, message: &str);
}

#[async_trait]
pub trait BtwSessionRuntime: Send + Sync {
    fn has_active_session(&self) -> bool;

    async fn prompt_interactive_agent(
        &self,
        agent_id: &str,
        prompt: &str,
    ) -> Result<(), BtwRuntimeError>;

    async fn cancel_interactive_agent(&self, agent_id: &str) -> Result<(), BtwRuntimeError>;
}

struct ActivePanel {
    agent_id: String,
    panel: SharedBtwPanel,
}

/// Controls the side-question panel and routes only its agent's events.
///
/// Original:
///   apps/kimi-code/src/tui/controllers/btw-panel.ts
///   BtwPanelController
pub struct BtwPanelController<H> {
    host: H,
    active: Option<ActivePanel>,
    panels_by_agent_id: HashMap<String, SharedBtwPanel>,
    can_use_scroll_keys: Arc<dyn Fn() -> bool + Send + Sync>,
    terminal_rows: Arc<dyn Fn() -> usize + Send + Sync>,
}

impl<H> BtwPanelController<H>
where
    H: BtwPanelHost,
{
    pub fn new(
        host: H,
        can_use_scroll_keys: Arc<dyn Fn() -> bool + Send + Sync>,
        terminal_rows: Arc<dyn Fn() -> usize + Send + Sync>,
    ) -> Self {
        Self {
            host,
            active: None,
            panels_by_agent_id: HashMap::new(),
            can_use_scroll_keys,
            terminal_rows,
        }
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    pub fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }

    // Original: BtwPanelController.open()
    pub fn open(&mut self, agent_id: &str, initial_prompt: &str) -> Option<BtwAsyncAction> {
        let can_use_scroll_keys = Arc::clone(&self.can_use_scroll_keys);
        let terminal_rows = Arc::clone(&self.terminal_rows);
        let panel = Arc::new(Mutex::new(BtwPanelComponent::new(BtwPanelOptions {
            markdown_options: MarkdownOptions::default(),
            can_use_scroll_keys: Box::new(move || can_use_scroll_keys()),
            on_prompt: Box::new(|_| {}),
            terminal_rows: Box::new(move || terminal_rows()),
        })));
        self.active = Some(ActivePanel {
            agent_id: agent_id.to_owned(),
            panel: Arc::clone(&panel),
        });
        self.panels_by_agent_id
            .insert(agent_id.to_owned(), Arc::clone(&panel));
        self.host.mount_btw_panel(Arc::clone(&panel));
        self.host.focus_editor();
        self.host.request_render(false);

        let mut panel_guard = lock_panel(&panel);
        panel_guard.submit(initial_prompt);
        panel_guard.is_running().then(|| BtwAsyncAction::Prompt {
            agent_id: agent_id.to_owned(),
            prompt: initial_prompt.trim().to_owned(),
        })
    }

    // Original: BtwPanelController.clear()
    pub fn clear(&mut self) -> Option<BtwAsyncAction> {
        let action = self.active.as_ref().and_then(|active| {
            should_cancel_on_unmount(&lock_panel(&active.panel)).then(|| BtwAsyncAction::Cancel {
                agent_id: active.agent_id.clone(),
            })
        });
        self.active = None;
        self.panels_by_agent_id.clear();
        self.host.unmount_btw_panel(false);
        action
    }

    // Original: BtwPanelController.closeOrCancel()
    pub fn close_or_cancel(&mut self) -> BtwCloseResult {
        let Some(active) = self.active.take() else {
            return BtwCloseResult {
                closed: false,
                action: None,
            };
        };
        let should_cancel = should_cancel_on_unmount(&lock_panel(&active.panel));
        self.unregister_panel(&active.panel);
        self.host.unmount_btw_panel(true);
        self.host.focus_editor();
        BtwCloseResult {
            closed: true,
            action: should_cancel.then_some(BtwAsyncAction::Cancel {
                agent_id: active.agent_id,
            }),
        }
    }

    pub fn cancel_running(&self) -> Option<BtwAsyncAction> {
        let active = self.active.as_ref()?;
        lock_panel(&active.panel)
            .is_running()
            .then(|| BtwAsyncAction::Cancel {
                agent_id: active.agent_id.clone(),
            })
    }

    pub fn send_user_input(&mut self, text: &str) -> BtwSendResult {
        let Some(active) = self.active.as_ref() else {
            return BtwSendResult {
                handled: false,
                action: None,
            };
        };
        let mut panel = lock_panel(&active.panel);
        if panel.is_running() {
            self.host.set_editor_text(text);
            panel.add_transient_notice(BTW_BUSY_NOTICE);
            drop(panel);
            self.host.request_render(false);
            return BtwSendResult {
                handled: true,
                action: None,
            };
        }
        panel.submit(text);
        let action = panel.is_running().then(|| BtwAsyncAction::Prompt {
            agent_id: active.agent_id.clone(),
            prompt: text.trim().to_owned(),
        });
        drop(panel);
        self.host.focus_editor();
        self.host.request_render(false);
        BtwSendResult {
            handled: true,
            action,
        }
    }

    pub fn scroll(&mut self, direction: BtwScrollDirection) -> bool {
        let Some(active) = self.active.as_ref() else {
            return false;
        };
        if !lock_panel(&active.panel).scroll(direction) {
            return false;
        }
        self.host.request_render(false);
        true
    }

    // Original: BtwPanelController.routeEvent()
    pub fn route_event(&mut self, event: &Event) -> bool {
        let Some(panel) = self.panels_by_agent_id.get(&event.agent_id).cloned() else {
            return false;
        };
        let mut panel = lock_panel(&panel);
        match &event.event {
            AgentEvent::AssistantDelta { delta, .. } => panel.append_answer(delta),
            AgentEvent::ThinkingDelta { delta, .. } => panel.append_thinking(delta),
            AgentEvent::HookResult {
                hook_event,
                content,
                blocked,
                ..
            } => panel.append_answer(&format_hook_result_plain(&HookResultEvent {
                hook_event: hook_event.clone(),
                content: content.clone(),
                blocked: blocked.unwrap_or(false),
            })),
            AgentEvent::TurnEnded { reason, error, .. } => {
                if *reason == TurnEndReason::Completed {
                    panel.mark_done(None);
                } else {
                    panel.mark_failed(format_btw_turn_end(*reason, error.as_ref()));
                }
            }
            _ => return true,
        }
        drop(panel);
        self.host.request_render(false);
        true
    }

    /// Executes the session/harness side effect returned by a controller
    /// method. Top-level TUI code may spawn this future under its task owner.
    pub async fn execute_action(
        &mut self,
        runtime: &dyn BtwSessionRuntime,
        action: BtwAsyncAction,
    ) {
        match action {
            BtwAsyncAction::Prompt { agent_id, prompt } => {
                if !runtime.has_active_session() {
                    if let Some(panel) = self.panels_by_agent_id.get(&agent_id) {
                        lock_panel(panel).mark_failed(NO_ACTIVE_SESSION_MESSAGE);
                    }
                    self.host.request_render(false);
                    return;
                }
                if let Err(error) = runtime.prompt_interactive_agent(&agent_id, &prompt).await {
                    if let Some(panel) = self.panels_by_agent_id.get(&agent_id) {
                        lock_panel(panel).mark_failed(format!(
                            "Failed to send /btw prompt: {}",
                            format_error_message(error.as_ref())
                        ));
                    }
                    self.host.request_render(false);
                }
            }
            BtwAsyncAction::Cancel { agent_id } => {
                if runtime.has_active_session()
                    && let Err(error) = runtime.cancel_interactive_agent(&agent_id).await
                {
                    self.host.show_error(&format!(
                        "Failed to cancel /btw: {}",
                        format_error_message(error.as_ref())
                    ));
                }
            }
        }
    }

    fn unregister_panel(&mut self, panel: &SharedBtwPanel) {
        self.panels_by_agent_id
            .retain(|_, candidate| !Arc::ptr_eq(candidate, panel));
    }
}

fn lock_panel(panel: &SharedBtwPanel) -> MutexGuard<'_, BtwPanelComponent> {
    match panel.lock() {
        Ok(panel) => panel,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn should_cancel_on_unmount(panel: &BtwPanelComponent) -> bool {
    panel.is_running() || panel.is_empty()
}

fn format_btw_turn_end(reason: TurnEndReason, error: Option<&KimiErrorPayload>) -> String {
    if reason == TurnEndReason::Cancelled {
        return "Interrupted by user".to_owned();
    }
    if error.is_some_and(|error| error.code == "provider.filtered") {
        return "Provider safety policy blocked the response.".to_owned();
    }
    if let Some(error) = error {
        return format!("[{}] {}", error.code, error.message);
    }
    if reason == TurnEndReason::Blocked {
        return "Prompt hook blocked the request.".to_owned();
    }
    format!(
        "BTW turn ended with reason: {}",
        turn_end_reason_name(reason)
    )
}

fn turn_end_reason_name(reason: TurnEndReason) -> &'static str {
    match reason {
        TurnEndReason::Completed => "completed",
        TurnEndReason::Cancelled => "cancelled",
        TurnEndReason::Failed => "failed",
        TurnEndReason::Blocked => "blocked",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[derive(Default)]
    struct HostMock {
        mounted: Option<SharedBtwPanel>,
        editor_text: String,
        focused: usize,
        renders: Vec<bool>,
        errors: Vec<String>,
    }

    impl BtwPanelHost for HostMock {
        fn mount_btw_panel(&mut self, panel: SharedBtwPanel) {
            self.mounted = Some(panel);
        }

        fn unmount_btw_panel(&mut self, force_render: bool) {
            self.mounted = None;
            self.renders.push(force_render);
        }

        fn set_editor_text(&mut self, text: &str) {
            self.editor_text = text.to_owned();
        }

        fn focus_editor(&mut self) {
            self.focused += 1;
        }

        fn request_render(&mut self, force: bool) {
            self.renders.push(force);
        }

        fn show_error(&mut self, message: &str) {
            self.errors.push(message.to_owned());
        }
    }

    struct RuntimeMock {
        active: bool,
        prompt_error: bool,
        cancel_error: bool,
    }

    #[async_trait]
    impl BtwSessionRuntime for RuntimeMock {
        fn has_active_session(&self) -> bool {
            self.active
        }

        async fn prompt_interactive_agent(&self, _: &str, _: &str) -> Result<(), BtwRuntimeError> {
            if self.prompt_error {
                Err(Box::new(io::Error::other("prompt broke")))
            } else {
                Ok(())
            }
        }

        async fn cancel_interactive_agent(&self, _: &str) -> Result<(), BtwRuntimeError> {
            if self.cancel_error {
                Err(Box::new(io::Error::other("cancel broke")))
            } else {
                Ok(())
            }
        }
    }

    fn controller() -> BtwPanelController<HostMock> {
        BtwPanelController::new(HostMock::default(), Arc::new(|| true), Arc::new(|| 24))
    }

    fn event(agent_id: &str, event: AgentEvent) -> Event {
        Event {
            agent_id: agent_id.to_owned(),
            session_id: "session-1".to_owned(),
            event,
        }
    }

    #[test]
    fn opens_routes_streaming_events_and_finishes_the_turn() {
        let mut controller = controller();
        assert_eq!(
            controller.open("sub-1", " question "),
            Some(BtwAsyncAction::Prompt {
                agent_id: "sub-1".to_owned(),
                prompt: "question".to_owned(),
            })
        );
        assert!(controller.route_event(&event(
            "sub-1",
            AgentEvent::ThinkingDelta {
                turn_id: 1,
                delta: "hmm".to_owned(),
            }
        )));
        assert!(controller.route_event(&event(
            "sub-1",
            AgentEvent::AssistantDelta {
                turn_id: 1,
                delta: "answer".to_owned(),
            }
        )));
        assert!(controller.route_event(&event(
            "sub-1",
            AgentEvent::TurnEnded {
                turn_id: 1,
                reason: TurnEndReason::Completed,
                error: None,
                duration_ms: None,
            }
        )));
        assert!(controller.cancel_running().is_none());
        assert!(controller.host().mounted.is_some());
    }

    #[test]
    fn busy_input_is_restored_to_editor_and_close_requests_cancel() {
        let mut controller = controller();
        controller.open("sub-1", "first");
        let result = controller.send_user_input("second");
        assert!(result.handled && result.action.is_none());
        assert_eq!(controller.host().editor_text, "second");

        let close = controller.close_or_cancel();
        assert!(close.closed);
        assert_eq!(
            close.action,
            Some(BtwAsyncAction::Cancel {
                agent_id: "sub-1".to_owned()
            })
        );
        assert!(controller.host().mounted.is_none());
    }

    #[test]
    fn ignores_other_agents_and_formats_failed_turns() {
        let mut controller = controller();
        controller.open("sub-1", "first");
        assert!(!controller.route_event(&event(
            "sub-2",
            AgentEvent::AssistantDelta {
                turn_id: 1,
                delta: "wrong".to_owned(),
            }
        )));
        assert!(controller.route_event(&event(
            "sub-1",
            AgentEvent::TurnEnded {
                turn_id: 1,
                reason: TurnEndReason::Failed,
                error: Some(KimiErrorPayload {
                    code: "provider.filtered".to_owned(),
                    message: "raw".to_owned(),
                    name: None,
                    details: None,
                    retryable: false,
                    cause: None,
                }),
                duration_ms: None,
            }
        )));
        assert!(controller.cancel_running().is_none());
    }

    #[tokio::test]
    async fn async_actions_surface_missing_session_and_runtime_failures() {
        let mut controller = controller();
        let prompt = controller.open("sub-1", "first").expect("prompt action");
        controller
            .execute_action(
                &RuntimeMock {
                    active: false,
                    prompt_error: false,
                    cancel_error: false,
                },
                prompt,
            )
            .await;
        assert!(controller.cancel_running().is_none());

        let prompt = controller
            .send_user_input("retry")
            .action
            .expect("retry prompt");
        controller
            .execute_action(
                &RuntimeMock {
                    active: true,
                    prompt_error: true,
                    cancel_error: true,
                },
                prompt,
            )
            .await;
        controller
            .execute_action(
                &RuntimeMock {
                    active: true,
                    prompt_error: true,
                    cancel_error: true,
                },
                BtwAsyncAction::Cancel {
                    agent_id: "sub-1".to_owned(),
                },
            )
            .await;
        assert_eq!(
            controller.host().errors,
            ["Failed to cancel /btw: cancel broke"]
        );
    }

    #[test]
    fn clear_cancels_empty_or_running_panel_and_resets_registration() {
        let mut controller = controller();
        controller.open("sub-1", "");
        assert_eq!(
            controller.clear(),
            Some(BtwAsyncAction::Cancel {
                agent_id: "sub-1".to_owned()
            })
        );
        assert!(!controller.route_event(&event(
            "sub-1",
            AgentEvent::AssistantDelta {
                turn_id: 1,
                delta: "late".to_owned(),
            }
        )));
        assert!(!controller.close_or_cancel().closed);
    }
}
