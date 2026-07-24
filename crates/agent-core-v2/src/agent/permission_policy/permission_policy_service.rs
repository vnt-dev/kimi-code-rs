//! Ordered agent permission-policy evaluation and dynamic policy registration.
//!
//! Original: `agent/permissionPolicy/permissionPolicyService.ts`.

use std::{
    ops::Deref,
    sync::{Arc, Mutex},
};

use futures_util::future::BoxFuture;

use crate::{
    _base::di::{
        instantiation::ServiceIdentifier,
        lifecycle::{DisposableHandle, to_disposable},
    },
    agent::tool_executor::ResolvedToolExecutionHookContext,
};

use super::{PermissionPolicy, PermissionPolicyResult};

#[derive(Clone)]
pub struct PermissionPolicyEvaluation {
    pub policy_name: String,
    pub result: Arc<PermissionPolicyResult>,
}

pub trait AgentPermissionPolicyServiceContract: Send + Sync {
    fn evaluate<'a>(
        &'a self,
        context: &'a ResolvedToolExecutionHookContext,
    ) -> BoxFuture<'a, Option<PermissionPolicyEvaluation>>;
    fn register_policy(&self, policy: Arc<dyn PermissionPolicy>) -> DisposableHandle;
}

#[derive(Clone)]
pub struct AgentPermissionPolicyServiceHandle(pub Arc<dyn AgentPermissionPolicyServiceContract>);
impl Deref for AgentPermissionPolicyServiceHandle {
    type Target = dyn AgentPermissionPolicyServiceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}
pub const AGENT_PERMISSION_POLICY_SERVICE_ID: ServiceIdentifier<
    AgentPermissionPolicyServiceHandle,
> = ServiceIdentifier::new("agentPermissionPolicyService");

pub struct AgentPermissionPolicyService {
    policies: Vec<Arc<dyn PermissionPolicy>>,
    dynamic_policies: Arc<Mutex<Vec<Arc<dyn PermissionPolicy>>>>,
}

impl AgentPermissionPolicyService {
    pub fn new(policies: Vec<Arc<dyn PermissionPolicy>>) -> Self {
        Self {
            policies,
            dynamic_policies: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl AgentPermissionPolicyServiceContract for AgentPermissionPolicyService {
    // Original: AgentPermissionPolicyService.evaluate(). Dynamic policies are evaluated in reverse registration order.
    fn evaluate<'a>(
        &'a self,
        context: &'a ResolvedToolExecutionHookContext,
    ) -> BoxFuture<'a, Option<PermissionPolicyEvaluation>> {
        Box::pin(async move {
            let dynamic = self
                .dynamic_policies
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            for policy in dynamic.iter().chain(self.policies.iter()) {
                if let Some(result) = policy.evaluate(context).await {
                    return Some(PermissionPolicyEvaluation {
                        policy_name: policy.name().into(),
                        result: Arc::new(result),
                    });
                }
            }
            None
        })
    }
    fn register_policy(&self, policy: Arc<dyn PermissionPolicy>) -> DisposableHandle {
        self.dynamic_policies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(0, Arc::clone(&policy));
        let policies = Arc::clone(&self.dynamic_policies);
        to_disposable(move || {
            policies
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|candidate| !Arc::ptr_eq(candidate, &policy))
        })
    }
}
