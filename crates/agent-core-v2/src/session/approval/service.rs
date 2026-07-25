//! Typed approval facade over the session interaction kernel.
//!
//! Original: `session/approval/approvalService.ts`.

use std::{
    ops::Deref,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::{ServiceIdentifier, ServicesAccessorExt},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    session::interaction::{
        InteractionKind, InteractionOrigin, InteractionRequest, SESSION_INTERACTION_SERVICE_ID,
        SessionInteractionService, SessionInteractionServiceHandle,
    },
};

use super::{ApprovalRequest, ApprovalResponse};

pub type ApprovalServiceError = serde_json::Error;

#[async_trait]
pub trait SessionApprovalServiceContract: Send + Sync {
    async fn request(
        &self,
        request: ApprovalRequest,
    ) -> Result<ApprovalResponse, ApprovalServiceError>;
    async fn enqueue(&self, request: ApprovalRequest) -> ApprovalRequest;
    async fn decide(&self, id: &str, response: ApprovalResponse);
    async fn list_pending(&self) -> Vec<ApprovalRequest>;
}

#[derive(Clone)]
pub struct SessionApprovalServiceHandle(pub Arc<dyn SessionApprovalServiceContract>);

impl Deref for SessionApprovalServiceHandle {
    type Target = dyn SessionApprovalServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const SESSION_APPROVAL_SERVICE_ID: ServiceIdentifier<SessionApprovalServiceHandle> =
    ServiceIdentifier::new("sessionApprovalService");

pub struct SessionApprovalService {
    interaction: Arc<SessionInteractionService>,
}

impl SessionApprovalService {
    pub fn new(interaction: Arc<SessionInteractionService>) -> Self {
        Self { interaction }
    }

    // Original: request(). The dynamic interaction kernel must be decoded at
    // this typed boundary; an invalid response is represented as a Rust
    // deserialization error rather than escaping an unchecked value.
    pub async fn request(
        &self,
        request: ApprovalRequest,
    ) -> Result<ApprovalResponse, ApprovalServiceError> {
        let id = request_id(&request);
        let response = self
            .interaction
            .request(interaction_request(id, request))
            .await;
        serde_json::from_value(response)
    }

    // Original: enqueue().
    pub async fn enqueue(&self, request: ApprovalRequest) -> ApprovalRequest {
        let id = request_id(&request);
        self.interaction
            .enqueue(interaction_request(id.clone(), request.clone()))
            .await;
        ApprovalRequest {
            id: Some(id),
            ..request
        }
    }

    // Original: decide().
    pub async fn decide(&self, id: &str, response: ApprovalResponse) {
        self.interaction
            .respond(id, serde_json::to_value(response).unwrap_or_default())
            .await;
    }

    // Original: listPending().
    pub async fn list_pending(&self) -> Vec<ApprovalRequest> {
        self.interaction
            .list_pending(Some(InteractionKind::Approval))
            .await
            .into_iter()
            .filter_map(|interaction| serde_json::from_value(interaction.payload).ok())
            .collect()
    }

    pub fn interaction(&self) -> &Arc<SessionInteractionService> {
        &self.interaction
    }
}

#[async_trait]
impl SessionApprovalServiceContract for SessionApprovalService {
    async fn request(
        &self,
        request: ApprovalRequest,
    ) -> Result<ApprovalResponse, ApprovalServiceError> {
        SessionApprovalService::request(self, request).await
    }

    async fn enqueue(&self, request: ApprovalRequest) -> ApprovalRequest {
        SessionApprovalService::enqueue(self, request).await
    }

    async fn decide(&self, id: &str, response: ApprovalResponse) {
        SessionApprovalService::decide(self, id, response).await;
    }

    async fn list_pending(&self) -> Vec<ApprovalRequest> {
        SessionApprovalService::list_pending(self).await
    }
}

pub fn register_session_approval_service() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_APPROVAL_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let interaction: SessionInteractionServiceHandle =
                (*accessor.get(SESSION_INTERACTION_SERVICE_ID)?).clone();
            let service: Arc<dyn SessionApprovalServiceContract> =
                Arc::new(SessionApprovalService::new(interaction.0));
            Ok(SessionApprovalServiceHandle(service))
        }),
        InstantiationType::Eager,
        "approval",
    );
}

fn interaction_request(id: String, request: ApprovalRequest) -> InteractionRequest {
    InteractionRequest {
        id: Some(id),
        kind: InteractionKind::Approval,
        payload: serde_json::to_value(&request).unwrap_or_default(),
        origin: Some(InteractionOrigin {
            agent_id: request.agent_id.clone(),
            turn_id: request.turn_id,
        }),
    }
}

// Original: requestId(). `toolCallId` takes precedence over the timestamp;
// the timestamp intentionally preserves the source's millisecond resolution.
fn request_id(request: &ApprovalRequest) -> String {
    request
        .id
        .clone()
        .or_else(|| request.tool_call_id.clone())
        .unwrap_or_else(|| format!("{}:{}", request.tool_name, now_ms()))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use kimi_code_protocol::CommandLanguage;

    use super::*;
    use crate::{
        session::approval::{ApprovalDecision, ApprovalResponse, ApprovalScope},
        tool::ToolInputDisplay,
    };

    fn request(id: Option<&str>) -> ApprovalRequest {
        ApprovalRequest {
            id: id.map(str::to_owned),
            session_id: Some("session-1".into()),
            agent_id: Some("agent-1".into()),
            turn_id: Some(2.5),
            tool_call_id: Some("call-1".into()),
            tool_name: "Bash".into(),
            action: "run command".into(),
            display: ToolInputDisplay::Command {
                command: "git status".into(),
                cwd: None,
                description: None,
                language: Some(CommandLanguage::Bash),
            },
        }
    }

    #[tokio::test]
    async fn requests_enqueue_decide_and_list_pending_through_interaction_kernel() {
        let approvals = Arc::new(SessionApprovalService::new(Arc::new(
            SessionInteractionService::new(),
        )));
        let pending = {
            let approvals = Arc::clone(&approvals);
            tokio::spawn(async move { approvals.request(request(Some("approval-1"))).await })
        };
        tokio::task::yield_now().await;
        assert_eq!(
            approvals
                .list_pending()
                .await
                .iter()
                .map(|request| request.id.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("approval-1")]
        );
        assert_eq!(
            approvals.interaction().list_pending(None).await[0]
                .origin
                .turn_id,
            Some(2.5)
        );
        let response = ApprovalResponse {
            decision: ApprovalDecision::Approved,
            scope: Some(ApprovalScope::Session),
            feedback: Some("safe".into()),
            selected_label: None,
        };
        approvals.decide("approval-1", response.clone()).await;
        assert_eq!(pending.await.unwrap().unwrap(), response);

        let queued = approvals.enqueue(request(Some("approval-2"))).await;
        assert_eq!(queued.id.as_deref(), Some("approval-2"));
        approvals.decide("approval-2", response).await;
        assert!(approvals.list_pending().await.is_empty());
    }

    #[test]
    fn explicit_id_then_tool_call_id_choose_source_request_id_order() {
        let with_id = request(Some("explicit"));
        assert_eq!(request_id(&with_id), "explicit");
        let without_id = request(None);
        assert_eq!(request_id(&without_id), "call-1");
    }
}
