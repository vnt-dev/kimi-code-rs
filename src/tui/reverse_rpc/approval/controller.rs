use std::sync::Arc;

use tokio::sync::oneshot;

use crate::{
    cli::prompt_session::{ApprovalDecision, ApprovalResponse, ApprovalScope},
    tui::reverse_rpc::{
        base_controller::{ReverseRpcController, ReverseRpcUiHooks},
        types::ApprovalPanelData,
    },
};

pub struct ApprovalController {
    inner: ReverseRpcController<ApprovalPanelData, ApprovalResponse>,
}

impl Default for ApprovalController {
    fn default() -> Self {
        Self::new()
    }
}

impl ApprovalController {
    pub fn new() -> Self {
        Self {
            inner: ReverseRpcController::with_auto_resolve(
                |reason| ApprovalResponse {
                    decision: ApprovalDecision::Cancelled,
                    scope: None,
                    feedback: Some(reason.to_owned()),
                    selected_label: None,
                },
                |resolved: &ApprovalPanelData,
                 response: &ApprovalResponse,
                 queued: &ApprovalPanelData| {
                    (response.decision == ApprovalDecision::Approved
                        && response.scope == Some(ApprovalScope::Session)
                        && resolved.action == queued.action)
                        .then_some(ApprovalResponse {
                            decision: ApprovalDecision::Approved,
                            scope: Some(ApprovalScope::Session),
                            feedback: None,
                            selected_label: None,
                        })
                },
            ),
        }
    }

    pub fn set_ui_hooks(&mut self, hooks: Arc<dyn ReverseRpcUiHooks<ApprovalPanelData>>) {
        self.inner.set_ui_hooks(hooks);
    }

    pub fn show(&mut self, payload: ApprovalPanelData) -> oneshot::Receiver<ApprovalResponse> {
        self.inner.show(payload)
    }

    pub fn respond(&mut self, response: ApprovalResponse) {
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

    fn panel(id: &str, action: &str) -> ApprovalPanelData {
        ApprovalPanelData {
            id: id.to_owned(),
            tool_call_id: id.to_owned(),
            tool_name: "Bash".to_owned(),
            action: action.to_owned(),
            description: String::new(),
            display: Vec::new(),
            choices: Vec::new(),
        }
    }

    fn approved(scope: Option<ApprovalScope>) -> ApprovalResponse {
        ApprovalResponse {
            decision: ApprovalDecision::Approved,
            scope,
            feedback: None,
            selected_label: None,
        }
    }

    #[tokio::test]
    async fn session_approval_auto_resolves_only_same_action_requests() {
        let mut controller = ApprovalController::new();
        let first = controller.show(panel("tc-1", "run command: ls"));
        let second = controller.show(panel("tc-2", "run command: ls"));
        let third = controller.show(panel("tc-3", "edit src/x.ts"));
        let fourth = controller.show(panel("tc-4", "run command: ls"));
        let mut response = approved(Some(ApprovalScope::Session));
        response.feedback = Some("ok".to_owned());
        controller.respond(response.clone());
        assert_eq!(first.await.expect("first"), response);
        assert_eq!(
            second.await.expect("second"),
            approved(Some(ApprovalScope::Session))
        );
        assert_eq!(
            fourth.await.expect("fourth"),
            approved(Some(ApprovalScope::Session))
        );
        assert!(controller.has_pending());
        controller.respond(ApprovalResponse {
            decision: ApprovalDecision::Rejected,
            scope: None,
            feedback: None,
            selected_label: None,
        });
        assert_eq!(
            third.await.expect("third").decision,
            ApprovalDecision::Rejected
        );
    }

    #[tokio::test]
    async fn one_shot_approval_does_not_auto_resolve_and_cancel_has_reason() {
        let mut controller = ApprovalController::new();
        let first = controller.show(panel("tc-1", "run"));
        let second = controller.show(panel("tc-2", "run"));
        controller.respond(approved(None));
        assert_eq!(first.await.expect("first"), approved(None));
        assert!(controller.has_pending());
        controller.cancel_all("closed");
        let cancelled = second.await.expect("second");
        assert_eq!(cancelled.decision, ApprovalDecision::Cancelled);
        assert_eq!(cancelled.feedback.as_deref(), Some("closed"));
    }
}
