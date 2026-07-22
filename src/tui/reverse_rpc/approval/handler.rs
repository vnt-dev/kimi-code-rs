use std::sync::Arc;

use tokio::sync::Mutex;

use crate::cli::prompt_session::{
    ApprovalDecision, ApprovalHandler, ApprovalRequest, ApprovalResponse,
};

use super::{adapter::adapt_approval_request, controller::ApprovalController};

pub type ApprovalResponseObserver = Arc<dyn Fn(&ApprovalRequest, &ApprovalResponse) + Send + Sync>;

// Original:
//   apps/kimi-code/src/tui/reverse-rpc/approval/handler.ts
//   createApprovalRequestHandler()
pub fn create_approval_request_handler(
    controller: Arc<Mutex<ApprovalController>>,
    on_response: Option<ApprovalResponseObserver>,
) -> ApprovalHandler {
    Arc::new(move |event| {
        let controller = Arc::clone(&controller);
        let on_response = on_response.clone();
        Box::pin(async move {
            let receiver = {
                let mut controller = controller.lock().await;
                controller.show(adapt_approval_request(&event))
            };
            let response = receiver.await.unwrap_or_else(|_| ApprovalResponse {
                decision: ApprovalDecision::Cancelled,
                scope: None,
                feedback: Some("approval handler failed".to_owned()),
                selected_label: None,
            });
            if let Some(observer) = on_response {
                observer(&event, &response);
            }
            response
        })
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use serde_json::json;

    use crate::cli::prompt_session::{ApprovalDecision, ApprovalScope};

    use super::*;

    fn request() -> ApprovalRequest {
        ApprovalRequest {
            turn_id: None,
            tool_call_id: "tc-1".to_owned(),
            tool_name: "Bash".to_owned(),
            action: "run command".to_owned(),
            display: json!({
                "kind": "generic",
                "summary": "run command",
                "detail": {"command": "rm -rf /tmp/cache", "cwd": "/tmp"}
            }),
        }
    }

    #[tokio::test]
    async fn adapts_request_waits_for_ui_and_reports_response() {
        let controller = Arc::new(Mutex::new(ApprovalController::new()));
        let observed = Arc::new(StdMutex::new(Vec::new()));
        let observer_values = Arc::clone(&observed);
        let handler = create_approval_request_handler(
            Arc::clone(&controller),
            Some(Arc::new(move |request, response| {
                observer_values
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((request.tool_call_id.clone(), response.clone()));
            })),
        );
        let task = tokio::spawn(handler(request()));
        tokio::task::yield_now().await;
        {
            let mut controller = controller.lock().await;
            assert!(controller.has_pending());
            controller.respond(ApprovalResponse {
                decision: ApprovalDecision::Approved,
                scope: Some(ApprovalScope::Session),
                feedback: Some("looks good".to_owned()),
                selected_label: None,
            });
        }
        let response = task.await.expect("handler task");
        assert_eq!(response.decision, ApprovalDecision::Approved);
        assert_eq!(response.scope, Some(ApprovalScope::Session));
        let values = observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].0, "tc-1");
    }

    #[tokio::test]
    async fn dropped_controller_response_returns_cancelled_fallback_and_notifies() {
        let controller = Arc::new(Mutex::new(ApprovalController::new()));
        let observed = Arc::new(StdMutex::new(Vec::new()));
        let observer_values = Arc::clone(&observed);
        let handler = create_approval_request_handler(
            Arc::clone(&controller),
            Some(Arc::new(move |_, response| {
                observer_values
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(response.clone());
            })),
        );
        let task = tokio::spawn(handler(request()));
        tokio::task::yield_now().await;
        *controller.lock().await = ApprovalController::new();
        let response = task.await.expect("handler task");
        assert_eq!(response.decision, ApprovalDecision::Cancelled);
        assert_eq!(
            response.feedback.as_deref(),
            Some("approval handler failed")
        );
        assert_eq!(
            observed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1
        );
    }
}
