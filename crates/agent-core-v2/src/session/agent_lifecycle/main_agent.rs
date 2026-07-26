//! Main-agent lifecycle convenience helper.
//!
//! Original: `session/agentLifecycle/mainAgent.ts`, `ensureMainAgent()`.

use crate::{
    _base::{di::scope::ScopeHandle, lifecycle::lifecycle_machine::BoxError},
    session::agent_lifecycle::{
        AGENT_LIFECYCLE_SERVICE_ID, AgentScopeHandle, CreateAgentOptions, MAIN_AGENT_ID,
    },
};

/// Returns the conventional main agent, creating it through the session's
/// lifecycle service when necessary. As in the source, the conventional ID
/// overrides any value supplied by the caller while all other options pass
/// through unchanged.
pub async fn ensure_main_agent(
    session: &ScopeHandle,
    options: Option<CreateAgentOptions>,
) -> Result<AgentScopeHandle, BoxError> {
    let lifecycle = session
        .get(AGENT_LIFECYCLE_SERVICE_ID)
        .map_err(|error| Box::new(error) as BoxError)?;
    let mut options = options.unwrap_or_default();
    options.agent_id = Some(MAIN_AGENT_ID.into());
    lifecycle.create(options).await
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };

    use futures_util::{FutureExt, future::BoxFuture};

    use crate::{
        _base::{
            di::{
                lifecycle::{Disposable, DisposeResult},
                scope::{Scope, ScopeOptions},
                service_collection::ServiceCollection,
            },
            event::Event,
        },
        agent::permission_policy::PermissionMode,
        session::agent_lifecycle::{
            AgentLifecycleServiceContract, AgentLifecycleServiceHandle, AgentListFilter,
            ForkAgentOptions,
        },
    };

    use super::*;

    struct Lifecycle {
        returned: AgentScopeHandle,
        options: Mutex<Vec<CreateAgentOptions>>,
        disposed: AtomicBool,
    }

    impl Disposable for Lifecycle {
        fn dispose(&self) -> DisposeResult {
            self.disposed.store(true, Ordering::Release);
            Ok(())
        }
    }

    impl AgentLifecycleServiceContract for Lifecycle {
        fn on_did_create(&self) -> Event<AgentScopeHandle> {
            Event::none()
        }

        fn on_did_dispose(&self) -> Event<String> {
            Event::none()
        }

        fn create(
            &self,
            options: CreateAgentOptions,
        ) -> BoxFuture<'static, Result<AgentScopeHandle, BoxError>> {
            self.options.lock().unwrap().push(options);
            futures_util::future::ready(Ok(self.returned.clone())).boxed()
        }

        fn fork(
            &self,
            _: String,
            _: ForkAgentOptions,
        ) -> BoxFuture<'static, Result<AgentScopeHandle, BoxError>> {
            futures_util::future::pending().boxed()
        }

        fn get(&self, _: &str) -> Option<AgentScopeHandle> {
            None
        }

        fn list(&self, _: Option<&AgentListFilter>) -> Vec<AgentScopeHandle> {
            Vec::new()
        }

        fn broadcast_permission_mode(&self, _: PermissionMode) -> Result<(), BoxError> {
            Ok(())
        }

        fn remove(&self, _: String) -> BoxFuture<'static, Result<(), BoxError>> {
            futures_util::future::ready(Ok(())).boxed()
        }
    }

    #[tokio::test]
    async fn resolves_session_lifecycle_and_forces_the_conventional_main_id() {
        let returned = Scope::create_app(ScopeOptions {
            id: Some("returned-agent".into()),
            ..ScopeOptions::default()
        })
        .to_handle();
        let lifecycle = Arc::new(Lifecycle {
            returned: returned.clone(),
            options: Mutex::new(Vec::new()),
            disposed: AtomicBool::new(false),
        });
        let mut services = ServiceCollection::new();
        let handle: Arc<dyn AgentLifecycleServiceContract> = lifecycle.clone();
        services.set_instance(
            AGENT_LIFECYCLE_SERVICE_ID,
            Arc::new(AgentLifecycleServiceHandle(handle)),
        );
        let session = Scope::create_app(ScopeOptions {
            id: Some("session".into()),
            extra: services,
        })
        .to_handle();

        let options = CreateAgentOptions {
            agent_id: Some("wrong".into()),
            forked_from: Some("source".into()),
            ..CreateAgentOptions::default()
        };
        let created = ensure_main_agent(&session, Some(options)).await.unwrap();
        assert_eq!(created.id(), returned.id());
        let seen = lifecycle.options.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].agent_id.as_deref(), Some(MAIN_AGENT_ID));
        assert_eq!(seen[0].forked_from.as_deref(), Some("source"));
        drop(seen);
        session
            .get(AGENT_LIFECYCLE_SERVICE_ID)
            .unwrap()
            .dispose()
            .unwrap();
        assert!(lifecycle.disposed.load(Ordering::Acquire));
    }
}
