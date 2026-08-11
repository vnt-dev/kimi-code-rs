use std::{ops::Deref, sync::Arc};

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::{ServiceIdentifier, ServicesAccessorExt},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    wire::{
        contract::{WIRE_SERVICE_ID, WireServiceHandle},
        wire_service::{WireService, WireServiceError},
    },
};

use super::{
    permission_rules_ops::{
        ADD_PERMISSION_RULES, PERMISSION_RULES_MODEL, RECORD_APPROVAL_RESULT, add_permission_rules,
        record_approval_result,
    },
    types::{PermissionApprovalResultRecord, PermissionRule},
};

pub trait AgentPermissionRulesServiceContract: Send + Sync {
    fn rules(&self) -> Vec<PermissionRule>;
    fn session_approval_rule_patterns(&self) -> Vec<String>;
    fn add_rules(&self, rules: &[PermissionRule]) -> Result<(), PermissionRulesServiceError>;
    fn record_approval_result(
        &self,
        record: PermissionApprovalResultRecord,
    ) -> Result<(), PermissionRulesServiceError>;
}

#[derive(Clone)]
pub struct AgentPermissionRulesServiceHandle(pub Arc<dyn AgentPermissionRulesServiceContract>);

impl Deref for AgentPermissionRulesServiceHandle {
    type Target = dyn AgentPermissionRulesServiceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const AGENT_PERMISSION_RULES_SERVICE_ID: ServiceIdentifier<AgentPermissionRulesServiceHandle> =
    ServiceIdentifier::new("agentPermissionRulesService");

#[derive(Debug, thiserror::Error)]
pub enum PermissionRulesServiceError {
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
    #[error(transparent)]
    Wire(#[from] WireServiceError),
}

pub struct AgentPermissionRulesService {
    wire: Arc<WireService>,
}

impl AgentPermissionRulesService {
    // Original: permissionRulesService.ts, AgentPermissionRulesService.constructor().
    pub fn new(wire: Arc<WireService>) -> Self {
        std::sync::LazyLock::force(&ADD_PERMISSION_RULES);
        std::sync::LazyLock::force(&RECORD_APPROVAL_RESULT);
        Self { wire }
    }
}

// Original: permissionRulesService.ts, registerScopedService(..., Eager,
// "permissionRules").
pub fn register_agent_permission_rules_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_PERMISSION_RULES_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let wire: WireServiceHandle = (*accessor.get(WIRE_SERVICE_ID)?).clone();
            let service: Arc<dyn AgentPermissionRulesServiceContract> =
                Arc::new(AgentPermissionRulesService::new(wire.0));
            Ok(AgentPermissionRulesServiceHandle(service))
        }),
        InstantiationType::Eager,
        "permissionRules",
    );
}

impl AgentPermissionRulesServiceContract for AgentPermissionRulesService {
    // Original: AgentPermissionRulesService.rules getter.
    fn rules(&self) -> Vec<PermissionRule> {
        self.wire.get_model(&PERMISSION_RULES_MODEL).rules
    }

    // Original: AgentPermissionRulesService.sessionApprovalRulePatterns getter.
    fn session_approval_rule_patterns(&self) -> Vec<String> {
        self.wire
            .get_model(&PERMISSION_RULES_MODEL)
            .session_approval_rule_patterns
    }

    // Original: AgentPermissionRulesService.addRules().
    fn add_rules(&self, rules: &[PermissionRule]) -> Result<(), PermissionRulesServiceError> {
        if rules.is_empty() {
            return Ok(());
        }
        self.wire
            .dispatch([add_permission_rules(rules.to_vec())?])?;
        Ok(())
    }

    // Original: AgentPermissionRulesService.recordApprovalResult().
    fn record_approval_result(
        &self,
        record: PermissionApprovalResultRecord,
    ) -> Result<(), PermissionRulesServiceError> {
        self.wire.dispatch([record_approval_result(record)?])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use futures_util::stream;
    use serde_json::Value;

    use super::*;
    use crate::{
        _base::di::lifecycle::disposable_none,
        agent::permission_rules::types::{PermissionRuleDecision, PermissionRuleScope},
        persistence::interface::append_log_store::{
            AppendLogError, AppendLogOptions, AppendLogStoreHandle, AppendLogStoreService,
            AppendLogValueStream,
        },
        session::approval::{ApprovalDecision, ApprovalResponse, ApprovalScope},
        wire::wire_service::{DomainEventPublisher, WireBlobService},
    };

    #[derive(Default)]
    struct MemoryLog;

    #[async_trait]
    impl AppendLogStoreService for MemoryLog {
        fn append_value(&self, _: &str, _: &str, _: Value, _: AppendLogOptions) {}
        fn read_values(&self, _: &str, _: &str) -> AppendLogValueStream {
            Box::pin(stream::empty())
        }
        async fn rewrite_values(
            &self,
            _: &str,
            _: &str,
            _: Vec<Value>,
        ) -> Result<(), AppendLogError> {
            Ok(())
        }
        async fn flush(&self) -> Result<(), AppendLogError> {
            Ok(())
        }
        async fn close(&self) -> Result<(), AppendLogError> {
            Ok(())
        }
        fn acquire(&self, _: &str, _: &str) -> crate::_base::di::lifecycle::DisposableHandle {
            disposable_none()
        }
    }

    struct IdentityBlobs;

    #[async_trait]
    impl WireBlobService for IdentityBlobs {
        async fn offload_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
            Ok(parts)
        }
        async fn load_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
            Ok(parts)
        }
    }

    struct NoopEvents;

    impl DomainEventPublisher for NoopEvents {
        fn publish(&self, _: Value) {}
    }

    fn setup() -> (Arc<WireService>, AgentPermissionRulesService) {
        let wire = Arc::new(WireService::new(
            "agents/permission-rules-test",
            AppendLogStoreHandle(Arc::new(MemoryLog)),
            Arc::new(IdentityBlobs),
            Arc::new(NoopEvents),
        ));
        let service = AgentPermissionRulesService::new(Arc::clone(&wire));
        (wire, service)
    }

    fn rule(pattern: &str) -> PermissionRule {
        PermissionRule {
            decision: PermissionRuleDecision::Allow,
            scope: PermissionRuleScope::User,
            pattern: pattern.into(),
            reason: None,
        }
    }

    #[tokio::test]
    async fn adds_rules_in_order_and_returns_defensive_snapshots() {
        let (wire, service) = setup();
        service.add_rules(&[]).unwrap();
        service
            .add_rules(&[rule("Read"), rule("Bash(git *)")])
            .unwrap();
        let mut snapshot = service.rules();
        snapshot.clear();
        assert_eq!(
            service
                .rules()
                .iter()
                .map(|rule| rule.pattern.as_str())
                .collect::<Vec<_>>(),
            ["Read", "Bash(git *)"]
        );
        wire.flush().await.unwrap();
    }

    #[tokio::test]
    async fn records_only_unique_approved_session_patterns() {
        let (wire, service) = setup();
        let record = PermissionApprovalResultRecord {
            turn_id: crate::agent::TurnId::new(1),
            tool_call_id: "call-1".into(),
            tool_name: "Bash".into(),
            action: "run".into(),
            session_approval_rule: Some("Bash(git *)".into()),
            result: ApprovalResponse {
                decision: ApprovalDecision::Approved,
                scope: Some(ApprovalScope::Session),
                feedback: None,
                selected_label: None,
            },
        };
        service.record_approval_result(record.clone()).unwrap();
        service.record_approval_result(record).unwrap();
        let mut snapshot = service.session_approval_rule_patterns();
        assert_eq!(snapshot, ["Bash(git *)"]);
        snapshot.clear();
        assert_eq!(service.session_approval_rule_patterns(), ["Bash(git *)"]);
        wire.flush().await.unwrap();
    }
}
