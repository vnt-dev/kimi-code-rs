use std::{collections::HashMap, error::Error, fmt};

use async_trait::async_trait;

use super::{
    options::{CliOptions, UiMode, validate_options},
    update::types::UpdatePreflightResult,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MainCommandOutcome {
    pub headless_completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MainCommandDisposition {
    Completed(MainCommandOutcome),
    Exit { code: i32, stderr: Option<String> },
}

#[derive(Debug)]
pub struct MainCommandRuntimeError(Box<dyn Error + Send + Sync>);

impl MainCommandRuntimeError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for MainCommandRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for MainCommandRuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[async_trait]
pub trait MainCommandRuntime: Send + Sync {
    async fn run_update_preflight(
        &self,
        version: &str,
        is_tty: Option<bool>,
    ) -> Result<UpdatePreflightResult, MainCommandRuntimeError>;

    async fn run_prompt(
        &self,
        options: &CliOptions,
        version: &str,
    ) -> Result<(), MainCommandRuntimeError>;

    async fn run_shell(
        &self,
        options: &CliOptions,
        version: &str,
    ) -> Result<(), MainCommandRuntimeError>;
}

// Original:
//   apps/kimi-code/src/main.ts
//   handleMainCommand()
//
// Rust adaptation:
//   Process termination is represented as a disposition. The eventual binary
//   entrypoint remains the sole owner of stderr and process exit, matching the
//   source method's reusable-handler intent without terminating an embedding
//   Rust process from library code.
pub async fn handle_main_command<R>(
    runtime: &R,
    options: &CliOptions,
    version: &str,
    environment: &HashMap<String, String>,
) -> Result<MainCommandDisposition, MainCommandRuntimeError>
where
    R: MainCommandRuntime + ?Sized,
{
    let validated = match validate_options(options, environment) {
        Ok(validated) => validated,
        Err(error) => {
            return Ok(MainCommandDisposition::Exit {
                code: 1,
                stderr: Some(format!("error: {error}\n")),
            });
        }
    };

    let preflight = runtime
        .run_update_preflight(
            version,
            (validated.ui_mode == UiMode::Print).then_some(false),
        )
        .await?;
    if preflight == UpdatePreflightResult::Exit {
        return Ok(MainCommandDisposition::Exit {
            code: 0,
            stderr: None,
        });
    }

    match validated.ui_mode {
        UiMode::Print => {
            runtime.run_prompt(&validated.options, version).await?;
            Ok(MainCommandDisposition::Completed(MainCommandOutcome {
                headless_completed: true,
            }))
        }
        UiMode::Shell => {
            runtime.run_shell(&validated.options, version).await?;
            Ok(MainCommandDisposition::Completed(MainCommandOutcome {
                headless_completed: false,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Preflight {
            version: String,
            is_tty: Option<bool>,
        },
        Prompt {
            prompt: Option<String>,
            version: String,
        },
        Shell {
            version: String,
        },
    }

    struct RuntimeMock {
        preflight: UpdatePreflightResult,
        calls: Mutex<Vec<Call>>,
    }

    impl RuntimeMock {
        fn continuing() -> Self {
            Self {
                preflight: UpdatePreflightResult::Continue,
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl MainCommandRuntime for RuntimeMock {
        async fn run_update_preflight(
            &self,
            version: &str,
            is_tty: Option<bool>,
        ) -> Result<UpdatePreflightResult, MainCommandRuntimeError> {
            self.calls.lock().expect("calls").push(Call::Preflight {
                version: version.to_owned(),
                is_tty,
            });
            Ok(self.preflight)
        }

        async fn run_prompt(
            &self,
            options: &CliOptions,
            version: &str,
        ) -> Result<(), MainCommandRuntimeError> {
            self.calls.lock().expect("calls").push(Call::Prompt {
                prompt: options.prompt.clone(),
                version: version.to_owned(),
            });
            Ok(())
        }

        async fn run_shell(
            &self,
            _: &CliOptions,
            version: &str,
        ) -> Result<(), MainCommandRuntimeError> {
            self.calls.lock().expect("calls").push(Call::Shell {
                version: version.to_owned(),
            });
            Ok(())
        }
    }

    #[tokio::test]
    async fn runs_preflight_before_the_interactive_shell() {
        let runtime = RuntimeMock::continuing();

        let disposition = handle_main_command(
            &runtime,
            &CliOptions::default(),
            "0.0.1-alpha.2",
            &HashMap::new(),
        )
        .await
        .expect("main command");

        assert_eq!(
            disposition,
            MainCommandDisposition::Completed(MainCommandOutcome {
                headless_completed: false
            })
        );
        assert_eq!(
            runtime.calls.lock().expect("calls").as_slice(),
            [
                Call::Preflight {
                    version: "0.0.1-alpha.2".to_owned(),
                    is_tty: None,
                },
                Call::Shell {
                    version: "0.0.1-alpha.2".to_owned(),
                }
            ]
        );
    }

    #[tokio::test]
    async fn runs_print_preflight_non_interactively_then_reports_headless_completion() {
        let runtime = RuntimeMock::continuing();
        let options = CliOptions {
            prompt: Some("explain the repo".to_owned()),
            ..CliOptions::default()
        };

        let disposition = handle_main_command(&runtime, &options, "0.0.1-alpha.2", &HashMap::new())
            .await
            .expect("main command");

        assert_eq!(
            disposition,
            MainCommandDisposition::Completed(MainCommandOutcome {
                headless_completed: true
            })
        );
        assert_eq!(
            runtime.calls.lock().expect("calls").as_slice(),
            [
                Call::Preflight {
                    version: "0.0.1-alpha.2".to_owned(),
                    is_tty: Some(false),
                },
                Call::Prompt {
                    prompt: Some("explain the repo".to_owned()),
                    version: "0.0.1-alpha.2".to_owned(),
                }
            ]
        );
    }

    #[tokio::test]
    async fn returns_exit_zero_without_starting_a_ui_when_preflight_requests_exit() {
        let runtime = RuntimeMock {
            preflight: UpdatePreflightResult::Exit,
            calls: Mutex::new(Vec::new()),
        };

        let disposition = handle_main_command(
            &runtime,
            &CliOptions::default(),
            "0.0.1-alpha.2",
            &HashMap::new(),
        )
        .await
        .expect("main command");

        assert_eq!(
            disposition,
            MainCommandDisposition::Exit {
                code: 0,
                stderr: None
            }
        );
        assert!(matches!(
            runtime.calls.lock().expect("calls").as_slice(),
            [Call::Preflight { .. }]
        ));
    }

    #[tokio::test]
    async fn reports_option_conflicts_before_running_preflight() {
        let runtime = RuntimeMock::continuing();
        let options = CliOptions {
            prompt: Some("work".to_owned()),
            yolo: true,
            ..CliOptions::default()
        };

        let disposition = handle_main_command(&runtime, &options, "0.0.1-alpha.2", &HashMap::new())
            .await
            .expect("validation disposition");

        assert_eq!(
            disposition,
            MainCommandDisposition::Exit {
                code: 1,
                stderr: Some("error: Cannot combine --prompt with --yolo.\n".to_owned())
            }
        );
        assert!(runtime.calls.lock().expect("calls").is_empty());
    }
}
