use std::{error::Error, fmt};

use async_trait::async_trait;

use crate::sdk::types::{SessionSummary, ShellEnvironment};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviousSessionSummary {
    pub work_dir: String,
    pub session_id: String,
    pub session_dir: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSessionInput {
    pub id: String,
    pub version: String,
    pub install_source: String,
    pub shell_env: ShellEnvironment,
    pub output_path: Option<String>,
    pub include_global_log: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSessionResult {
    pub zip_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportOptions {
    pub yes: bool,
    pub include_global_log: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportDisposition {
    Completed,
    Cancelled,
    Exit(i32),
}

#[derive(Debug)]
pub struct ExportRuntimeError(Box<dyn Error + Send + Sync>);

impl ExportRuntimeError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for ExportRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for ExportRuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[async_trait]
pub trait ExportRuntime: Send + Sync {
    async fn list_sessions(
        &self,
        work_dir: &str,
    ) -> Result<Vec<SessionSummary>, ExportRuntimeError>;

    async fn export_session(
        &self,
        input: ExportSessionInput,
    ) -> Result<ExportSessionResult, ExportRuntimeError>;

    async fn confirm_previous_session(
        &self,
        summary: PreviousSessionSummary,
    ) -> Result<bool, ExportRuntimeError>;

    async fn install_source(&self) -> Result<String, ExportRuntimeError>;

    fn shell_environment(&self) -> ShellEnvironment;

    fn version(&self) -> &str;

    fn current_dir(&self) -> String;

    fn write_stdout(&self, text: &str);

    fn write_stderr(&self, text: &str);
}

// Original:
//   apps/kimi-code/src/cli/sub/export.ts
//   handleExport()
pub async fn handle_export(
    runtime: &dyn ExportRuntime,
    session_id: Option<&str>,
    output: Option<&str>,
    options: ExportOptions,
) -> Result<ExportDisposition, ExportRuntimeError> {
    let requested_id = normalize_optional_session_id(session_id);
    let previous = if requested_id.is_none() {
        runtime
            .list_sessions(&runtime.current_dir())
            .await?
            .into_iter()
            .next()
    } else {
        None
    };

    let resolved_id = if let Some(requested_id) = requested_id {
        requested_id.to_owned()
    } else {
        let Some(previous) = previous else {
            runtime.write_stderr("No previous session found to export.\n");
            return Ok(ExportDisposition::Exit(1));
        };
        if !options.yes
            && !runtime
                .confirm_previous_session(to_previous_session_summary(&previous))
                .await?
        {
            runtime.write_stdout("Export cancelled.\n");
            return Ok(ExportDisposition::Cancelled);
        }
        previous.id
    };

    let result = async {
        let install_source = runtime.install_source().await?;
        runtime
            .export_session(ExportSessionInput {
                id: resolved_id,
                version: runtime.version().to_owned(),
                install_source,
                shell_env: runtime.shell_environment(),
                output_path: output.map(str::to_owned),
                include_global_log: options.include_global_log.then_some(true),
            })
            .await
    }
    .await;
    match result {
        Ok(result) => {
            runtime.write_stdout(&format!("{}\n", result.zip_path));
            Ok(ExportDisposition::Completed)
        }
        Err(error) => {
            runtime.write_stderr(&format!("{error}\n"));
            Ok(ExportDisposition::Exit(1))
        }
    }
}

fn normalize_optional_session_id(session_id: Option<&str>) -> Option<&str> {
    session_id
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty())
}

fn to_previous_session_summary(summary: &SessionSummary) -> PreviousSessionSummary {
    PreviousSessionSummary {
        work_dir: summary.work_dir.clone(),
        session_id: summary.id.clone(),
        session_dir: summary.session_dir.clone(),
        title: summary.title.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Debug, Clone, Copy)]
    struct TestError(&'static str);

    impl fmt::Display for TestError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.0)
        }
    }

    impl Error for TestError {}

    struct RuntimeMock {
        sessions: Vec<SessionSummary>,
        confirm: bool,
        export_error: bool,
        listed_dirs: Mutex<Vec<String>>,
        confirmations: Mutex<Vec<PreviousSessionSummary>>,
        export_inputs: Mutex<Vec<ExportSessionInput>>,
        stdout: Mutex<String>,
        stderr: Mutex<String>,
    }

    impl RuntimeMock {
        fn new() -> Self {
            Self {
                sessions: Vec::new(),
                confirm: true,
                export_error: false,
                listed_dirs: Mutex::new(Vec::new()),
                confirmations: Mutex::new(Vec::new()),
                export_inputs: Mutex::new(Vec::new()),
                stdout: Mutex::new(String::new()),
                stderr: Mutex::new(String::new()),
            }
        }
    }

    #[async_trait]
    impl ExportRuntime for RuntimeMock {
        async fn list_sessions(
            &self,
            work_dir: &str,
        ) -> Result<Vec<SessionSummary>, ExportRuntimeError> {
            self.listed_dirs
                .lock()
                .expect("listed dirs")
                .push(work_dir.to_owned());
            Ok(self.sessions.clone())
        }

        async fn export_session(
            &self,
            input: ExportSessionInput,
        ) -> Result<ExportSessionResult, ExportRuntimeError> {
            self.export_inputs
                .lock()
                .expect("export inputs")
                .push(input.clone());
            if self.export_error {
                Err(ExportRuntimeError::new(TestError("session was not found")))
            } else {
                Ok(ExportSessionResult {
                    zip_path: input
                        .output_path
                        .unwrap_or_else(|| format!("/tmp/{}.zip", input.id)),
                })
            }
        }

        async fn confirm_previous_session(
            &self,
            summary: PreviousSessionSummary,
        ) -> Result<bool, ExportRuntimeError> {
            self.confirmations
                .lock()
                .expect("confirmations")
                .push(summary);
            Ok(self.confirm)
        }

        async fn install_source(&self) -> Result<String, ExportRuntimeError> {
            Ok("npm-global".to_owned())
        }

        fn shell_environment(&self) -> ShellEnvironment {
            ShellEnvironment {
                term: Some("xterm-256color".to_owned()),
                shell: Some("/bin/zsh".to_owned()),
                ..ShellEnvironment::default()
            }
        }

        fn version(&self) -> &str {
            "1.0.0-test"
        }

        fn current_dir(&self) -> String {
            "/work".to_owned()
        }

        fn write_stdout(&self, text: &str) {
            self.stdout.lock().expect("stdout").push_str(text);
        }

        fn write_stderr(&self, text: &str) {
            self.stderr.lock().expect("stderr").push_str(text);
        }
    }

    fn summary(id: &str, title: Option<&str>) -> SessionSummary {
        SessionSummary {
            id: id.to_owned(),
            title: title.map(str::to_owned),
            last_prompt: None,
            work_dir: "/work".to_owned(),
            session_dir: format!("/sessions/{id}"),
            created_at: Some(1.0),
            updated_at: Some(2.0),
            archived: None,
            metadata: None,
            additional_dirs: None,
        }
    }

    #[tokio::test]
    async fn named_session_skips_lookup_and_delegates_all_export_context() {
        let runtime = RuntimeMock::new();
        let disposition = handle_export(
            &runtime,
            Some("  ses_test123456  "),
            Some("/tmp/out.zip"),
            ExportOptions {
                yes: false,
                include_global_log: true,
            },
        )
        .await
        .expect("export");

        assert_eq!(disposition, ExportDisposition::Completed);
        assert!(runtime.listed_dirs.lock().expect("listed").is_empty());
        assert_eq!(
            runtime.export_inputs.lock().expect("inputs").as_slice(),
            [ExportSessionInput {
                id: "ses_test123456".to_owned(),
                version: "1.0.0-test".to_owned(),
                install_source: "npm-global".to_owned(),
                shell_env: runtime.shell_environment(),
                output_path: Some("/tmp/out.zip".to_owned()),
                include_global_log: Some(true),
            }]
        );
        assert_eq!(
            runtime.stdout.lock().expect("stdout").as_str(),
            "/tmp/out.zip\n"
        );
    }

    #[tokio::test]
    async fn no_previous_session_reports_exit_one_without_exporting() {
        let runtime = RuntimeMock::new();
        let disposition = handle_export(
            &runtime,
            None,
            None,
            ExportOptions {
                yes: false,
                include_global_log: true,
            },
        )
        .await
        .expect("handled missing session");
        assert_eq!(disposition, ExportDisposition::Exit(1));
        assert_eq!(
            runtime.listed_dirs.lock().expect("listed").as_slice(),
            ["/work"]
        );
        assert!(
            runtime
                .stderr
                .lock()
                .expect("stderr")
                .contains("No previous session")
        );
        assert!(runtime.export_inputs.lock().expect("inputs").is_empty());
    }

    #[tokio::test]
    async fn previous_session_confirmation_can_cancel_with_title_context() {
        let mut runtime = RuntimeMock::new();
        runtime.sessions = vec![summary("ses_confirm", Some("Prod debug"))];
        runtime.confirm = false;
        let disposition = handle_export(
            &runtime,
            Some("   "),
            None,
            ExportOptions {
                yes: false,
                include_global_log: true,
            },
        )
        .await
        .expect("cancel export");
        assert_eq!(disposition, ExportDisposition::Cancelled);
        assert_eq!(
            runtime
                .confirmations
                .lock()
                .expect("confirmations")
                .as_slice(),
            [PreviousSessionSummary {
                work_dir: "/work".to_owned(),
                session_id: "ses_confirm".to_owned(),
                session_dir: "/sessions/ses_confirm".to_owned(),
                title: Some("Prod debug".to_owned()),
            }]
        );
        assert!(runtime.export_inputs.lock().expect("inputs").is_empty());
        assert!(
            runtime
                .stdout
                .lock()
                .expect("stdout")
                .contains("Export cancelled")
        );
    }

    #[tokio::test]
    async fn yes_skips_confirmation_and_false_global_log_is_omitted() {
        let mut runtime = RuntimeMock::new();
        runtime.sessions = vec![summary("ses_yes", None)];
        let disposition = handle_export(
            &runtime,
            None,
            None,
            ExportOptions {
                yes: true,
                include_global_log: false,
            },
        )
        .await
        .expect("export");
        assert_eq!(disposition, ExportDisposition::Completed);
        assert!(
            runtime
                .confirmations
                .lock()
                .expect("confirmations")
                .is_empty()
        );
        assert_eq!(
            runtime.export_inputs.lock().expect("inputs")[0].include_global_log,
            None
        );
    }

    #[tokio::test]
    async fn export_error_is_printed_and_converted_to_exit_one() {
        let mut runtime = RuntimeMock::new();
        runtime.export_error = true;
        let disposition = handle_export(
            &runtime,
            Some("ses_missing"),
            None,
            ExportOptions {
                yes: false,
                include_global_log: true,
            },
        )
        .await
        .expect("handled export error");
        assert_eq!(disposition, ExportDisposition::Exit(1));
        assert!(runtime.stderr.lock().expect("stderr").contains("not found"));
    }
}
