use std::sync::{Arc, Mutex, MutexGuard};

use super::{
    approval::ApprovalController,
    base_controller::ReverseRpcUiHooks,
    modal_coordinator::{ReverseRpcModalCoordinator, ReverseRpcModalOwner, ReverseRpcModalUiHooks},
    question::QuestionController,
    types::{ApprovalPanelData, QuestionPanelData},
};

struct ApprovalModalHooks {
    coordinator: Arc<Mutex<ReverseRpcModalCoordinator>>,
}

impl ReverseRpcUiHooks<ApprovalPanelData> for ApprovalModalHooks {
    fn show_panel(&self, payload: &ApprovalPanelData) {
        lock_coordinator(&self.coordinator).show_approval(payload.clone());
    }

    fn hide_panel(&self) {
        lock_coordinator(&self.coordinator).hide(ReverseRpcModalOwner::Approval);
    }
}

struct QuestionModalHooks {
    coordinator: Arc<Mutex<ReverseRpcModalCoordinator>>,
}

impl ReverseRpcUiHooks<QuestionPanelData> for QuestionModalHooks {
    fn show_panel(&self, payload: &QuestionPanelData) {
        lock_coordinator(&self.coordinator).show_question(payload.clone());
    }

    fn hide_panel(&self) {
        lock_coordinator(&self.coordinator).hide(ReverseRpcModalOwner::Question);
    }
}

pub struct ReverseRpcRegistration {
    coordinator: Arc<Mutex<ReverseRpcModalCoordinator>>,
}

impl ReverseRpcRegistration {
    pub fn dispose(&self) {
        lock_coordinator(&self.coordinator).clear();
    }
}

// Original:
//   apps/kimi-code/src/tui/reverse-rpc/index.ts
//   registerReverseRPCHandlers()
pub fn register_reverse_rpc_handlers(
    approval_controller: &mut ApprovalController,
    question_controller: &mut QuestionController,
    ui_hooks: Arc<dyn ReverseRpcModalUiHooks>,
) -> ReverseRpcRegistration {
    let coordinator = Arc::new(Mutex::new(ReverseRpcModalCoordinator::new(ui_hooks)));
    approval_controller.set_ui_hooks(Arc::new(ApprovalModalHooks {
        coordinator: Arc::clone(&coordinator),
    }));
    question_controller.set_ui_hooks(Arc::new(QuestionModalHooks {
        coordinator: Arc::clone(&coordinator),
    }));
    ReverseRpcRegistration { coordinator }
}

fn lock_coordinator(
    coordinator: &Mutex<ReverseRpcModalCoordinator>,
) -> MutexGuard<'_, ReverseRpcModalCoordinator> {
    coordinator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crate::{
        cli::prompt_session::{ApprovalDecision, ApprovalResponse},
        tui::reverse_rpc::QuestionPanelResponse,
    };

    use super::*;

    #[derive(Default)]
    struct UiEvents(Mutex<Vec<String>>);

    impl UiEvents {
        fn values(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl ReverseRpcModalUiHooks for UiEvents {
        fn show_approval_panel(&self, payload: &ApprovalPanelData) {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("show-approval:{}", payload.id));
        }

        fn hide_approval_panel(&self) {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push("hide-approval".to_owned());
        }

        fn show_question_dialog(&self, payload: &QuestionPanelData) {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("show-question:{}", payload.id));
        }

        fn hide_question_dialog(&self) {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push("hide-question".to_owned());
        }
    }

    fn approval(id: &str) -> ApprovalPanelData {
        ApprovalPanelData {
            id: id.to_owned(),
            tool_call_id: id.to_owned(),
            tool_name: "Bash".to_owned(),
            action: "run".to_owned(),
            description: String::new(),
            display: Vec::new(),
            choices: Vec::new(),
        }
    }

    fn question(id: &str) -> QuestionPanelData {
        QuestionPanelData {
            id: id.to_owned(),
            tool_call_id: id.to_owned(),
            questions: Vec::new(),
        }
    }

    fn approved() -> ApprovalResponse {
        ApprovalResponse {
            decision: ApprovalDecision::Approved,
            scope: None,
            feedback: None,
            selected_label: None,
        }
    }

    #[tokio::test]
    async fn question_waits_behind_active_approval_then_is_shown() {
        let ui = Arc::new(UiEvents::default());
        let mut approvals = ApprovalController::new();
        let mut questions = QuestionController::new();
        let _registration =
            register_reverse_rpc_handlers(&mut approvals, &mut questions, ui.clone());
        let approval_pending = approvals.show(approval("approval-1"));
        let question_pending = questions.show(question("question-1"));
        assert_eq!(ui.values(), ["show-approval:approval-1"]);
        approvals.respond(approved());
        assert_eq!(approval_pending.await.expect("approval"), approved());
        assert_eq!(
            ui.values(),
            [
                "show-approval:approval-1",
                "hide-approval",
                "show-question:question-1"
            ]
        );
        questions.respond(QuestionPanelResponse {
            answers: vec![Some("answer".to_owned())],
            method: None,
        });
        assert_eq!(
            question_pending.await.expect("question").answers,
            [Some("answer".to_owned())]
        );
        assert_eq!(
            ui.values().last().map(String::as_str),
            Some("hide-question")
        );
    }

    #[tokio::test]
    async fn approval_waits_behind_active_question_then_is_shown() {
        let ui = Arc::new(UiEvents::default());
        let mut approvals = ApprovalController::new();
        let mut questions = QuestionController::new();
        let _registration =
            register_reverse_rpc_handlers(&mut approvals, &mut questions, ui.clone());
        let question_pending = questions.show(question("question-1"));
        let approval_pending = approvals.show(approval("approval-1"));
        assert_eq!(ui.values(), ["show-question:question-1"]);
        questions.respond(QuestionPanelResponse::cancelled());
        question_pending.await.expect("question");
        assert_eq!(
            ui.values(),
            [
                "show-question:question-1",
                "hide-question",
                "show-approval:approval-1"
            ]
        );
        approvals.respond(approved());
        assert_eq!(approval_pending.await.expect("approval"), approved());
    }

    #[tokio::test]
    async fn cancelling_queued_owner_removes_modal_without_hiding_it() {
        let ui = Arc::new(UiEvents::default());
        let mut approvals = ApprovalController::new();
        let mut questions = QuestionController::new();
        let _registration =
            register_reverse_rpc_handlers(&mut approvals, &mut questions, ui.clone());
        let approval_pending = approvals.show(approval("approval-1"));
        let question_pending = questions.show(question("question-1"));
        questions.cancel_all("closed");
        assert_eq!(
            question_pending.await.expect("question"),
            QuestionPanelResponse::cancelled()
        );
        approvals.respond(approved());
        approval_pending.await.expect("approval");
        assert_eq!(ui.values(), ["show-approval:approval-1", "hide-approval"]);
    }

    #[tokio::test]
    async fn dispose_hides_active_and_drops_queued_modal_without_showing_it() {
        let ui = Arc::new(UiEvents::default());
        let mut approvals = ApprovalController::new();
        let mut questions = QuestionController::new();
        let registration =
            register_reverse_rpc_handlers(&mut approvals, &mut questions, ui.clone());
        let approval_pending = approvals.show(approval("approval-1"));
        let question_pending = questions.show(question("question-1"));
        registration.dispose();
        assert_eq!(ui.values(), ["show-approval:approval-1", "hide-approval"]);
        approvals.cancel_all("closed");
        questions.cancel_all("closed");
        approval_pending.await.expect("approval");
        question_pending.await.expect("question");
        assert_eq!(ui.values(), ["show-approval:approval-1", "hide-approval"]);
    }
}
