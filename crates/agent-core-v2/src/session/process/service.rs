//! Session process runner over the host process boundary.
//!
//! Original: `packages/agent-core-v2/src/session/process/processRunnerService.ts`.

use std::{collections::HashMap, fmt, sync::Arc};

use async_trait::async_trait;

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::ServicesAccessorExt,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    os::interface::host_process::{
        HOST_PROCESS_SERVICE_ID, HostProcessOptions, HostProcessServiceHandle,
    },
    session::session_context::{SESSION_CONTEXT_ID, SessionContext},
};

use super::contract::{
    ProcessExecOptions, SESSION_PROCESS_RUNNER_SERVICE_ID, SessionProcess,
    SessionProcessRunnerContract, SessionProcessRunnerHandle, SessionProcessRunnerResult,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissingProcessCommandError;

impl fmt::Display for MissingProcessCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "SessionProcessRunner.exec(): at least one argument (the command to run) is required.",
        )
    }
}

impl std::error::Error for MissingProcessCommandError {}

pub struct SessionProcessRunner {
    context: Arc<SessionContext>,
    host_process: HostProcessServiceHandle,
}

impl SessionProcessRunner {
    // Original: SessionProcessRunner.constructor().
    pub fn new(context: Arc<SessionContext>, host_process: HostProcessServiceHandle) -> Self {
        Self {
            context,
            host_process,
        }
    }

    // Original: SessionProcessRunner._buildExecEnv(). Reading the current
    // process environment is intentionally skipped when no overlay is given.
    fn build_exec_env(
        invocation_env: Option<HashMap<String, String>>,
    ) -> Option<HashMap<String, String>> {
        let invocation_env = invocation_env?;
        let mut environment = std::env::vars().collect::<HashMap<_, _>>();
        environment.extend(invocation_env);
        Some(environment)
    }
}

#[async_trait]
impl SessionProcessRunnerContract for SessionProcessRunner {
    // Original: SessionProcessRunner.exec(). Spawn remains a single awaited
    // host call; no concurrent task or retry is introduced.
    async fn exec(
        &self,
        args: &[String],
        options: Option<ProcessExecOptions>,
    ) -> SessionProcessRunnerResult<SessionProcess> {
        let Some((command, arguments)) = args.split_first() else {
            return Err(Box::new(MissingProcessCommandError));
        };
        let options = options.unwrap_or_default();
        let cwd = options.cwd.unwrap_or_else(|| self.context.cwd.clone());
        let env = Self::build_exec_env(options.env);
        self.host_process
            .spawn(
                command,
                arguments,
                HostProcessOptions {
                    cwd: Some(cwd),
                    env,
                    ..HostProcessOptions::default()
                },
            )
            .await
            .map_err(|error| Box::new(error) as _)
    }
}

pub fn register_session_process_runner() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_PROCESS_RUNNER_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let context = accessor.get(SESSION_CONTEXT_ID)?;
            let host_process = accessor.get(HOST_PROCESS_SERVICE_ID)?;
            let service: Arc<dyn SessionProcessRunnerContract> =
                Arc::new(SessionProcessRunner::new(context, (*host_process).clone()));
            Ok(SessionProcessRunnerHandle(service))
        }),
        InstantiationType::Eager,
        "process",
    );
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::{
        io::{empty, sink},
        sync::Mutex,
    };

    use crate::os::interface::host_process::{
        HostProcess, HostProcessError, HostProcessService, ProcessSignal, SharedProcessReader,
        SharedProcessWriter,
    };

    use super::*;

    struct DummyProcess;

    #[async_trait]
    impl HostProcess for DummyProcess {
        fn pid(&self) -> i64 {
            7
        }

        fn exit_code(&self) -> Option<i32> {
            None
        }

        fn stdin(&self) -> SharedProcessWriter {
            Arc::new(Mutex::new(Box::new(sink())))
        }

        fn stdout(&self) -> SharedProcessReader {
            Arc::new(Mutex::new(Box::new(empty())))
        }

        fn stderr(&self) -> SharedProcessReader {
            Arc::new(Mutex::new(Box::new(empty())))
        }

        async fn wait(&self) -> Result<i32, HostProcessError> {
            Ok(0)
        }

        async fn kill(&self, _: Option<ProcessSignal>) -> Result<(), HostProcessError> {
            Ok(())
        }

        fn dispose(&self) {}
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct SpawnCall {
        command: String,
        arguments: Vec<String>,
        options: HostProcessOptions,
    }

    #[derive(Default)]
    struct StubHostProcess {
        calls: StdMutex<Vec<SpawnCall>>,
        count: AtomicUsize,
    }

    #[async_trait]
    impl HostProcessService for StubHostProcess {
        async fn spawn(
            &self,
            command: &str,
            args: &[String],
            options: HostProcessOptions,
        ) -> Result<Arc<dyn HostProcess>, HostProcessError> {
            self.count.fetch_add(1, Ordering::Relaxed);
            self.calls.lock().push(SpawnCall {
                command: command.into(),
                arguments: args.into(),
                options,
            });
            Ok(Arc::new(DummyProcess))
        }
    }

    fn context() -> Arc<SessionContext> {
        Arc::new(crate::session::session_context::make_session_context(
            crate::session::session_context::SessionContextInput {
                session_id: "session".into(),
                workspace_id: "workspace".into(),
                session_dir: "/sessions/workspace/session".into(),
                session_scope: "sessions/workspace/session".into(),
                cwd: "/seeded".into(),
                meta_scope: None,
            },
        ))
    }

    #[tokio::test]
    async fn exec_splits_command_uses_seeded_cwd_and_inherits_environment() {
        let host = Arc::new(StubHostProcess::default());
        let service = SessionProcessRunner::new(context(), HostProcessServiceHandle(host.clone()));
        let process = service
            .exec(&["git".into(), "status".into()], None)
            .await
            .unwrap();

        assert_eq!(process.pid(), 7);
        let calls = host.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].command, "git");
        assert_eq!(calls[0].arguments, ["status"]);
        assert_eq!(calls[0].options.cwd.as_deref(), Some("/seeded"));
        assert_eq!(calls[0].options.env, None);
    }

    #[tokio::test]
    async fn invocation_options_override_cwd_and_overlay_current_environment() {
        let host = Arc::new(StubHostProcess::default());
        let service = SessionProcessRunner::new(context(), HostProcessServiceHandle(host.clone()));
        let unique_key = format!("KIMI_SESSION_TEST_{}", uuid::Uuid::new_v4());
        service
            .exec(
                &["pwd".into()],
                Some(ProcessExecOptions {
                    cwd: Some("/override".into()),
                    env: Some(HashMap::from([(unique_key.clone(), "value".into())])),
                }),
            )
            .await
            .unwrap();

        let calls = host.calls.lock();
        assert_eq!(calls[0].options.cwd.as_deref(), Some("/override"));
        assert_eq!(
            calls[0]
                .options
                .env
                .as_ref()
                .and_then(|environment| environment.get(&unique_key))
                .map(String::as_str),
            Some("value")
        );
    }

    #[tokio::test]
    async fn empty_arguments_fail_before_host_spawn() {
        let host = Arc::new(StubHostProcess::default());
        let service = SessionProcessRunner::new(context(), HostProcessServiceHandle(host.clone()));
        let error = match service.exec(&[], None).await {
            Ok(_) => panic!("empty arguments must fail"),
            Err(error) => error,
        };
        assert_eq!(error.to_string(), MissingProcessCommandError.to_string());
        assert_eq!(host.count.load(Ordering::Relaxed), 0);
    }
}
