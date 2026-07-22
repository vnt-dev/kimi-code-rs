use std::sync::Arc;

use tokio::sync::oneshot;

use crate::tui::reverse_rpc::{
    base_controller::{ReverseRpcController, ReverseRpcUiHooks},
    types::{QuestionPanelData, QuestionPanelResponse},
};

pub struct QuestionController {
    inner: ReverseRpcController<QuestionPanelData, QuestionPanelResponse>,
}

impl Default for QuestionController {
    fn default() -> Self {
        Self::new()
    }
}

impl QuestionController {
    pub fn new() -> Self {
        Self {
            inner: ReverseRpcController::new(|_| QuestionPanelResponse::cancelled()),
        }
    }

    pub fn set_ui_hooks(&mut self, hooks: Arc<dyn ReverseRpcUiHooks<QuestionPanelData>>) {
        self.inner.set_ui_hooks(hooks);
    }

    pub fn show(&mut self, payload: QuestionPanelData) -> oneshot::Receiver<QuestionPanelResponse> {
        self.inner.show(payload)
    }

    pub fn respond(&mut self, response: QuestionPanelResponse) {
        self.inner.respond(response);
    }

    pub fn cancel_all(&mut self, reason: &str) {
        self.inner.cancel_all(reason);
    }

    pub fn has_pending(&self) -> bool {
        self.inner.has_pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancel_resolves_with_empty_answers() {
        let mut controller = QuestionController::new();
        let pending = controller.show(QuestionPanelData {
            id: "req-1".to_owned(),
            tool_call_id: "tc-1".to_owned(),
            questions: Vec::new(),
        });
        controller.cancel_all("closed");
        assert_eq!(
            pending.await.expect("response"),
            QuestionPanelResponse::cancelled()
        );
    }
}
