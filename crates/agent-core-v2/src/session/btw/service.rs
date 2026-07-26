//! Side-question (`btw`) session service implementation.
//!
//! Original: `packages/agent-core-v2/src/session/btw/btwService.ts`.

use std::sync::Arc;

use futures_util::future::BoxFuture;

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::ServicesAccessorExt,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    agent::{
        context_memory::PromptOrigin,
        permission_policy::{
            AGENT_PERMISSION_POLICY_SERVICE_ID, policies::DenyAllPermissionPolicy,
        },
        system_reminder::AGENT_SYSTEM_REMINDER_SERVICE_ID,
    },
    session::agent_lifecycle::{
        AGENT_LIFECYCLE_SERVICE_ID, AgentLifecycleServiceHandle, ForkAgentOptions, MAIN_AGENT_ID,
    },
};

use super::{
    SESSION_BTW_SERVICE_ID, SIDE_QUESTION_SYSTEM_REMINDER, SessionBtwServiceContract,
    SessionBtwServiceHandle, TOOL_CALL_DISABLED_MESSAGE,
};

pub struct SessionBtwService {
    lifecycle: AgentLifecycleServiceHandle,
}

impl SessionBtwService {
    pub fn new(lifecycle: AgentLifecycleServiceHandle) -> Self {
        Self { lifecycle }
    }
}

impl SessionBtwServiceContract for SessionBtwService {
    fn start(
        &self,
    ) -> BoxFuture<'static, Result<String, crate::_base::lifecycle::lifecycle_machine::BoxError>>
    {
        let lifecycle = self.lifecycle.clone();
        Box::pin(async move {
            let child = lifecycle
                .fork(MAIN_AGENT_ID.into(), ForkAgentOptions::default())
                .await?;
            child
                .get(AGENT_SYSTEM_REMINDER_SERVICE_ID)?
                .append_system_reminder(
                    SIDE_QUESTION_SYSTEM_REMINDER,
                    PromptOrigin::SystemTrigger { name: "btw".into() },
                )?;
            child
                .get(AGENT_PERMISSION_POLICY_SERVICE_ID)?
                .register_policy(Arc::new(DenyAllPermissionPolicy::new(Some(
                    TOOL_CALL_DISABLED_MESSAGE.into(),
                ))));
            Ok(child.id().into())
        })
    }
}

// Original: registerScopedService(..., SessionBtwService, Eager,
// "session-btw").
pub fn register_session_btw_service() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_BTW_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let lifecycle = accessor.get(AGENT_LIFECYCLE_SERVICE_ID)?;
            let service: Arc<dyn SessionBtwServiceContract> =
                Arc::new(SessionBtwService::new((*lifecycle).clone()));
            Ok(SessionBtwServiceHandle(service))
        }),
        InstantiationType::Eager,
        "session-btw",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::di::scope::{
        clear_scoped_registry_for_tests, get_scoped_service_descriptors,
    };

    #[test]
    fn registration_matches_the_eager_session_scoped_source_binding() {
        clear_scoped_registry_for_tests();
        register_session_btw_service();
        let entries = get_scoped_service_descriptors(LifecycleScope::Session);
        assert!(entries.iter().any(|entry| {
            entry.id.to_string() == SESSION_BTW_SERVICE_ID.to_string()
                && !entry.descriptor.supports_delayed_instantiation
                && entry.domain == "session-btw"
        }));
        clear_scoped_registry_for_tests();
    }
}
