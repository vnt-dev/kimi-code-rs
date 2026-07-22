use std::{collections::VecDeque, sync::Arc};

use tokio::sync::oneshot;

pub trait ReverseRpcUiHooks<TPayload>: Send + Sync {
    fn show_panel(&self, payload: &TPayload);
    fn hide_panel(&self);
}

struct Pending<TPayload, TResponse> {
    payload: TPayload,
    sender: oneshot::Sender<TResponse>,
}

type CancelResponse<TResponse> = dyn Fn(&str) -> TResponse + Send + Sync;
type AutoResolve<TPayload, TResponse> =
    dyn Fn(&TPayload, &TResponse, &TPayload) -> Option<TResponse> + Send + Sync;

/// FIFO coordinator for reverse-RPC requests that wait for a TUI response.
///
/// Original:
///   apps/kimi-code/src/tui/reverse-rpc/base-controller.ts
///   ReverseRpcController
///
/// Rust adaptation:
///   `show` returns a Tokio one-shot receiver, allowing the core-facing async
///   handler to await the UI without holding a mutable controller borrow.
pub struct ReverseRpcController<TPayload, TResponse> {
    ui_hooks: Option<Arc<dyn ReverseRpcUiHooks<TPayload>>>,
    current: Option<Pending<TPayload, TResponse>>,
    queue: VecDeque<Pending<TPayload, TResponse>>,
    create_cancel_response: Arc<CancelResponse<TResponse>>,
    auto_resolve_for: Arc<AutoResolve<TPayload, TResponse>>,
}

impl<TPayload, TResponse> ReverseRpcController<TPayload, TResponse>
where
    TResponse: Clone,
{
    pub fn new(create_cancel_response: impl Fn(&str) -> TResponse + Send + Sync + 'static) -> Self {
        Self::with_auto_resolve(create_cancel_response, |_, _, _| None)
    }

    pub fn with_auto_resolve(
        create_cancel_response: impl Fn(&str) -> TResponse + Send + Sync + 'static,
        auto_resolve_for: impl Fn(&TPayload, &TResponse, &TPayload) -> Option<TResponse>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            ui_hooks: None,
            current: None,
            queue: VecDeque::new(),
            create_cancel_response: Arc::new(create_cancel_response),
            auto_resolve_for: Arc::new(auto_resolve_for),
        }
    }

    // Original: ReverseRpcController.setUIHooks()
    pub fn set_ui_hooks(&mut self, hooks: Arc<dyn ReverseRpcUiHooks<TPayload>>) {
        self.ui_hooks = Some(hooks);
    }

    // Original: ReverseRpcController.show()
    pub fn show(&mut self, payload: TPayload) -> oneshot::Receiver<TResponse> {
        let (sender, receiver) = oneshot::channel();
        let entry = Pending { payload, sender };
        if self.current.is_none() {
            if let Some(hooks) = &self.ui_hooks {
                hooks.show_panel(&entry.payload);
            }
            self.current = Some(entry);
        } else {
            self.queue.push_back(entry);
        }
        receiver
    }

    // Original: ReverseRpcController.respond()
    pub fn respond(&mut self, response: TResponse) {
        let Some(pending) = self.current.take() else {
            return;
        };
        let resolved_payload = pending.payload;
        let _ = pending.sender.send(response.clone());
        self.drain_auto_resolved(&resolved_payload, &response);
        self.advance_or_hide();
    }

    // Original: ReverseRpcController.cancelAll()
    pub fn cancel_all(&mut self, reason: &str) {
        let current = self.current.take();
        let queue = std::mem::take(&mut self.queue);
        if let Some(hooks) = &self.ui_hooks {
            hooks.hide_panel();
        }
        for entry in current.into_iter().chain(queue) {
            let _ = entry.sender.send((self.create_cancel_response)(reason));
        }
    }

    pub fn has_pending(&self) -> bool {
        self.current.is_some() || !self.queue.is_empty()
    }

    fn advance_or_hide(&mut self) {
        let Some(next) = self.queue.pop_front() else {
            if let Some(hooks) = &self.ui_hooks {
                hooks.hide_panel();
            }
            return;
        };
        if let Some(hooks) = &self.ui_hooks {
            hooks.show_panel(&next.payload);
        }
        self.current = Some(next);
    }

    fn drain_auto_resolved(&mut self, resolved_payload: &TPayload, response: &TResponse) {
        let mut remaining = VecDeque::with_capacity(self.queue.len());
        while let Some(entry) = self.queue.pop_front() {
            match (self.auto_resolve_for)(resolved_payload, response, &entry.payload) {
                Some(auto_response) => {
                    let _ = entry.sender.send(auto_response);
                }
                None => remaining.push_back(entry),
            }
        }
        self.queue = remaining;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct Hooks {
        shown: Mutex<Vec<String>>,
        hides: Mutex<usize>,
    }

    impl ReverseRpcUiHooks<String> for Hooks {
        fn show_panel(&self, payload: &String) {
            self.shown.lock().expect("shown").push(payload.clone());
        }

        fn hide_panel(&self) {
            *self.hides.lock().expect("hides") += 1;
        }
    }

    #[tokio::test]
    async fn shows_resolves_and_hides_one_request() {
        let hooks = Arc::new(Hooks::default());
        let mut controller = ReverseRpcController::new(|reason| format!("cancel:{reason}"));
        controller.set_ui_hooks(hooks.clone());
        let pending = controller.show("payload".to_owned());
        assert!(controller.has_pending());
        assert_eq!(*hooks.shown.lock().expect("shown"), ["payload"]);
        controller.respond("approved".to_owned());
        assert_eq!(pending.await.expect("response"), "approved");
        assert!(!controller.has_pending());
        assert_eq!(*hooks.hides.lock().expect("hides"), 1);
    }

    #[tokio::test]
    async fn concurrent_requests_are_presented_in_fifo_order_without_intermediate_hide() {
        let hooks = Arc::new(Hooks::default());
        let mut controller = ReverseRpcController::new(|reason| format!("cancel:{reason}"));
        controller.set_ui_hooks(hooks.clone());
        let first = controller.show("first".to_owned());
        let second = controller.show("second".to_owned());
        let third = controller.show("third".to_owned());
        assert_eq!(*hooks.shown.lock().expect("shown"), ["first"]);

        controller.respond("answer-first".to_owned());
        assert_eq!(first.await.expect("first"), "answer-first");
        assert_eq!(*hooks.shown.lock().expect("shown"), ["first", "second"]);
        assert_eq!(*hooks.hides.lock().expect("hides"), 0);
        controller.respond("answer-second".to_owned());
        assert_eq!(second.await.expect("second"), "answer-second");
        controller.respond("answer-third".to_owned());
        assert_eq!(third.await.expect("third"), "answer-third");
        assert_eq!(*hooks.hides.lock().expect("hides"), 1);
    }

    #[derive(Clone)]
    struct Payload {
        action: &'static str,
        id: &'static str,
    }

    #[tokio::test]
    async fn auto_resolves_matching_queued_requests() {
        let mut controller = ReverseRpcController::with_auto_resolve(
            |reason| format!("cancel:{reason}"),
            |resolved: &Payload, response: &String, queued: &Payload| {
                (response == "approve_all_same" && resolved.action == queued.action)
                    .then(|| format!("auto:{}", queued.id))
            },
        );
        let first = controller.show(Payload {
            action: "run",
            id: "a",
        });
        let second = controller.show(Payload {
            action: "run",
            id: "b",
        });
        let third = controller.show(Payload {
            action: "edit",
            id: "c",
        });
        let fourth = controller.show(Payload {
            action: "run",
            id: "d",
        });
        controller.respond("approve_all_same".to_owned());
        assert_eq!(first.await.expect("first"), "approve_all_same");
        assert_eq!(second.await.expect("second"), "auto:b");
        assert_eq!(fourth.await.expect("fourth"), "auto:d");
        assert!(controller.has_pending());
        controller.respond("approve_all_same".to_owned());
        assert_eq!(third.await.expect("third"), "approve_all_same");
        assert!(!controller.has_pending());
    }

    #[tokio::test]
    async fn cancel_all_resolves_current_and_every_queued_request() {
        let hooks = Arc::new(Hooks::default());
        let mut controller = ReverseRpcController::new(|reason| format!("cancel:{reason}"));
        controller.set_ui_hooks(hooks.clone());
        let first = controller.show("first".to_owned());
        let second = controller.show("second".to_owned());
        let third = controller.show("third".to_owned());
        controller.cancel_all("shutdown");
        assert_eq!(first.await.expect("first"), "cancel:shutdown");
        assert_eq!(second.await.expect("second"), "cancel:shutdown");
        assert_eq!(third.await.expect("third"), "cancel:shutdown");
        assert!(!controller.has_pending());
        assert_eq!(*hooks.hides.lock().expect("hides"), 1);
    }
}
