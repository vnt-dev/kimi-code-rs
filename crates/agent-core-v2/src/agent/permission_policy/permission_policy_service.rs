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
    agent::{
        permission_mode::AgentPermissionModeServiceContract,
        permission_rules::AgentPermissionRulesServiceContract, plan::AgentPlanServiceContract,
        swarm::AgentSwarmServiceContract, tool_executor::ResolvedToolExecutionHookContext,
    },
    app::telemetry::TelemetryServiceContract,
    os::interface::host_environment::HostEnvironment,
    session::workspace_context::SessionWorkspaceContextContract,
};

use super::{
    AgentSwarmExclusiveDenyPermissionPolicy, AutoModeApprovePermissionPolicy,
    AutoModeAskUserQuestionDenyPermissionPolicy, DefaultToolApprovePermissionPolicy,
    ExitPlanModeReviewAskPermissionPolicy, FallbackAskPermissionPolicy,
    GitControlPathAccessAskPermissionPolicy, GitCwdWriteApprovePermissionPolicy,
    GoalStartReviewAskPermissionPolicy, PermissionPolicy, PermissionPolicyResult,
    PlanModeGuardDenyPermissionPolicy, PlanModeToolApprovePermissionPolicy,
    SensitiveFileAccessAskPermissionPolicy, SessionApprovalHistoryPermissionPolicy,
    SwarmModeAgentSwarmApprovePermissionPolicy, UserConfiguredAllowPermissionPolicy,
    UserConfiguredAskPermissionPolicy, UserConfiguredDenyPermissionPolicy,
    YoloModeApprovePermissionPolicy,
};

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

    // Original: AgentPermissionPolicyService.constructor(). Rust receives the
    // already-resolved dependencies explicitly because the current DI core
    // cannot retrieve bare trait service identifiers.
    #[allow(clippy::too_many_arguments)]
    pub fn from_dependencies(
        mode: Arc<dyn AgentPermissionModeServiceContract>,
        rules: Arc<dyn AgentPermissionRulesServiceContract>,
        plan: Arc<dyn AgentPlanServiceContract>,
        swarm: Arc<dyn AgentSwarmServiceContract>,
        telemetry: Arc<dyn TelemetryServiceContract>,
        environment: Arc<dyn HostEnvironment>,
        workspace: Arc<dyn SessionWorkspaceContextContract>,
    ) -> Self {
        let policies: Vec<Arc<dyn PermissionPolicy>> = vec![
            Arc::new(AgentSwarmExclusiveDenyPermissionPolicy),
            Arc::new(AutoModeAskUserQuestionDenyPermissionPolicy::new(
                Arc::clone(&mode),
            )),
            Arc::new(PlanModeGuardDenyPermissionPolicy::new(Arc::clone(&plan))),
            Arc::new(UserConfiguredDenyPermissionPolicy::new(Arc::clone(&rules))),
            Arc::new(AutoModeApprovePermissionPolicy::new(Arc::clone(&mode))),
            Arc::new(SessionApprovalHistoryPermissionPolicy::new(Arc::clone(
                &rules,
            ))),
            Arc::new(UserConfiguredAskPermissionPolicy::new(Arc::clone(&rules))),
            Arc::new(UserConfiguredAllowPermissionPolicy::new(Arc::clone(&rules))),
            Arc::new(ExitPlanModeReviewAskPermissionPolicy::new(
                Arc::clone(&plan),
                Arc::clone(&mode),
                Arc::clone(&telemetry),
            )),
            Arc::new(GoalStartReviewAskPermissionPolicy::new(Arc::clone(&mode))),
            Arc::new(PlanModeToolApprovePermissionPolicy::new(Arc::clone(&plan))),
            Arc::new(SensitiveFileAccessAskPermissionPolicy),
            Arc::new(GitControlPathAccessAskPermissionPolicy::new(
                Arc::clone(&environment),
                Arc::clone(&workspace),
            )),
            Arc::new(YoloModeApprovePermissionPolicy::new(Arc::clone(&mode))),
            Arc::new(SwarmModeAgentSwarmApprovePermissionPolicy::new(Arc::clone(
                &swarm,
            ))),
            Arc::new(DefaultToolApprovePermissionPolicy),
            Arc::new(GitCwdWriteApprovePermissionPolicy::new(
                environment,
                workspace,
            )),
            Arc::new(FallbackAskPermissionPolicy),
        ];
        Self::new(policies)
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
