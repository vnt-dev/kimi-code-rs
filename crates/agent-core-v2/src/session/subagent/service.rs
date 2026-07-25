//! Session-scoped subagent run service.
//!
//! Original: `packages/agent-core-v2/src/session/subagent/subagentService.ts`,
//! `SessionSubagentService`.

use std::sync::Arc;

use futures_util::{FutureExt, future::BoxFuture};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        lifecycle::lifecycle_machine::BoxError,
    },
    agent::profile::AGENT_PROFILE_SERVICE_ID,
    session::{
        agent_lifecycle::{AGENT_LIFECYCLE_SERVICE_ID, AgentLifecycleServiceHandle},
        agent_profile_catalog::{
            SESSION_AGENT_PROFILE_CATALOG_ID, SessionAgentProfileCatalogHandle,
        },
    },
};

use super::{
    AgentRunHandle, AgentRunRequest, AgentTaskHooks, AgentTaskStopEmitter,
    AgentTaskStopHookContext, RunAgentOptions, SESSION_SUBAGENT_SERVICE_ID,
    SessionSubagentServiceContract, run_agent_turn,
};

pub struct SessionSubagentService {
    agent_lifecycle: AgentLifecycleServiceHandle,
    catalog: SessionAgentProfileCatalogHandle,
    hooks: AgentTaskHooks,
    stopped: AgentTaskStopEmitter,
}

impl SessionSubagentService {
    pub fn new(
        agent_lifecycle: AgentLifecycleServiceHandle,
        catalog: SessionAgentProfileCatalogHandle,
    ) -> Self {
        Self {
            agent_lifecycle,
            catalog,
            hooks: AgentTaskHooks::default(),
            stopped: AgentTaskStopEmitter::default(),
        }
    }
}

impl Disposable for SessionSubagentService {
    fn dispose(&self) -> DisposeResult {
        Ok(())
    }
}

impl SessionSubagentServiceContract for SessionSubagentService {
    fn hooks(&self) -> &AgentTaskHooks {
        &self.hooks
    }

    fn on_did_stop_agent_task(&self) -> crate::_base::event::Event<AgentTaskStopHookContext> {
        self.stopped.event()
    }

    fn run(
        &self,
        agent_id: String,
        request: AgentRunRequest,
        mut options: RunAgentOptions,
    ) -> BoxFuture<'static, Result<AgentRunHandle, BoxError>> {
        let lifecycle = self.agent_lifecycle.clone();
        let catalog = self.catalog.clone();
        async move {
            let target = lifecycle
                .get(&agent_id)
                .ok_or_else(|| Box::new(UnknownAgentError(agent_id.clone())) as BoxError)?;
            if options.summary_policy.is_none() {
                let profile = target.get(AGENT_PROFILE_SERVICE_ID)?;
                let data = profile.data()?;
                options.summary_policy = data
                    .config
                    .profile_name
                    .as_deref()
                    .and_then(|name| catalog.get(name))
                    .and_then(|profile| profile.summary_policy.clone());
            }
            run_agent_turn(&target, request, options).await
        }
        .boxed()
    }

    fn notify_agent_task_stopped(&self, context: AgentTaskStopHookContext) {
        self.stopped.fire(&context);
    }
}

/// Original `registerScopedService(..., LifecycleScope.Session, Eager,
/// "subagent")`.
pub fn register_session_subagent_service() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_SUBAGENT_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let lifecycle = accessor.get(AGENT_LIFECYCLE_SERVICE_ID)?;
            let catalog = accessor.get(SESSION_AGENT_PROFILE_CATALOG_ID)?;
            let service: Arc<dyn SessionSubagentServiceContract> = Arc::new(
                SessionSubagentService::new((*lifecycle).clone(), (*catalog).clone()),
            );
            Ok(super::SessionSubagentServiceHandle(service))
        }),
        InstantiationType::Eager,
        "subagent",
    );
}

#[derive(Debug, thiserror::Error)]
#[error("Agent \"{0}\" does not exist")]
struct UnknownAgentError(String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_uses_session_scope_and_source_domain() {
        crate::_base::di::scope::clear_scoped_registry_for_tests();
        register_session_subagent_service();
        let entries =
            crate::_base::di::scope::get_scoped_service_descriptors(LifecycleScope::Session);
        assert!(entries.iter().any(|entry| {
            entry.id.to_string() == SESSION_SUBAGENT_SERVICE_ID.to_string()
                && entry.domain == "subagent"
        }));
        crate::_base::di::scope::clear_scoped_registry_for_tests();
    }

    #[test]
    fn unknown_agent_error_preserves_source_message() {
        assert_eq!(
            UnknownAgentError("child".into()).to_string(),
            "Agent \"child\" does not exist"
        );
    }
}
