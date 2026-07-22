use std::{collections::HashMap, error::Error, fmt};

use async_trait::async_trait;

use super::{
    commands::{CommandInvocation, ParsedCommand},
    main_command::{
        MainCommandDisposition, MainCommandOutcome, MainCommandRuntime, MainCommandRuntimeError,
        handle_main_command,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubcommandOutcome {
    pub exit_code: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntrypointDisposition {
    Completed {
        exit_code: i32,
        headless_completed: bool,
    },
    Exit {
        code: i32,
        stderr: Option<String>,
    },
}

#[derive(Debug)]
pub struct EntrypointRuntimeError(Box<dyn Error + Send + Sync>);

impl EntrypointRuntimeError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for EntrypointRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for EntrypointRuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[derive(Debug)]
pub struct EntrypointFailure {
    operation: &'static str,
    source: Box<dyn Error + Send + Sync>,
}

impl EntrypointFailure {
    fn main(operation: &'static str, error: MainCommandRuntimeError) -> Self {
        Self {
            operation,
            source: Box::new(error),
        }
    }

    fn subcommand(operation: &'static str, error: EntrypointRuntimeError) -> Self {
        Self {
            operation,
            source: Box::new(error),
        }
    }

    pub fn operation(&self) -> &'static str {
        self.operation
    }
}

impl fmt::Display for EntrypointFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.source.fmt(formatter)
    }
}

impl Error for EntrypointFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[async_trait]
pub trait EntrypointRuntime: MainCommandRuntime {
    async fn run_subcommand(
        &self,
        command: &CommandInvocation,
        version: &str,
    ) -> Result<SubcommandOutcome, EntrypointRuntimeError>;
}

// Original:
//   apps/kimi-code/src/main.ts
//   main()
//
// Rust adaptation:
//   Commander callbacks become one awaited typed dispatch. Process-owned I/O
//   and termination stay in `src/main.rs`, while this function remains fully
//   testable and never terminates an embedding process.
pub async fn run_entrypoint<R>(
    runtime: &R,
    parsed: &ParsedCommand,
    version: &str,
    environment: &HashMap<String, String>,
) -> Result<EntrypointDisposition, EntrypointFailure>
where
    R: EntrypointRuntime + ?Sized,
{
    let Some(command) = parsed.command.as_ref() else {
        let operation = if parsed.options.prompt.is_some() {
            "run prompt"
        } else {
            "start shell"
        };
        return handle_main_command(runtime, &parsed.options, version, environment)
            .await
            .map(map_main_disposition)
            .map_err(|error| EntrypointFailure::main(operation, error));
    };

    runtime
        .run_subcommand(command, version)
        .await
        .map(|outcome| EntrypointDisposition::Completed {
            exit_code: outcome.exit_code,
            headless_completed: false,
        })
        .map_err(|error| EntrypointFailure::subcommand(command.operation(), error))
}

fn map_main_disposition(disposition: MainCommandDisposition) -> EntrypointDisposition {
    match disposition {
        MainCommandDisposition::Completed(MainCommandOutcome { headless_completed }) => {
            EntrypointDisposition::Completed {
                exit_code: 0,
                headless_completed,
            }
        }
        MainCommandDisposition::Exit { code, stderr } => {
            EntrypointDisposition::Exit { code, stderr }
        }
    }
}

impl CommandInvocation {
    pub fn operation(&self) -> &'static str {
        match self {
            Self::Export(_) => "export session",
            Self::Provider(_) => "manage providers",
            Self::Acp(_) => "start ACP server",
            Self::Web(_) => "start web server",
            Self::Server(_) => "run deprecated server command",
            Self::Login => "login",
            Self::Doctor(_) => "run doctor",
            Self::Vis(_) => "start visualizer",
            Self::Migrate => "run migration",
            Self::Upgrade => "upgrade",
            Self::PluginRunNode(_) => "run plugin node entry",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::cli::{
        commands::parse_command_from, options::CliOptions, update::types::UpdatePreflightResult,
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Preflight(Option<bool>),
        Prompt,
        Shell,
        Subcommand(&'static str),
    }

    #[derive(Default)]
    struct RuntimeMock {
        calls: Mutex<Vec<Call>>,
    }

    #[async_trait]
    impl MainCommandRuntime for RuntimeMock {
        async fn run_update_preflight(
            &self,
            _: &str,
            is_tty: Option<bool>,
        ) -> Result<UpdatePreflightResult, MainCommandRuntimeError> {
            self.calls
                .lock()
                .expect("calls")
                .push(Call::Preflight(is_tty));
            Ok(UpdatePreflightResult::Continue)
        }

        async fn run_prompt(&self, _: &CliOptions, _: &str) -> Result<(), MainCommandRuntimeError> {
            self.calls.lock().expect("calls").push(Call::Prompt);
            Ok(())
        }

        async fn run_shell(&self, _: &CliOptions, _: &str) -> Result<(), MainCommandRuntimeError> {
            self.calls.lock().expect("calls").push(Call::Shell);
            Ok(())
        }
    }

    #[async_trait]
    impl EntrypointRuntime for RuntimeMock {
        async fn run_subcommand(
            &self,
            command: &CommandInvocation,
            _: &str,
        ) -> Result<SubcommandOutcome, EntrypointRuntimeError> {
            self.calls
                .lock()
                .expect("calls")
                .push(Call::Subcommand(command.operation()));
            Ok(SubcommandOutcome { exit_code: 7 })
        }
    }

    fn parse(arguments: &[&str]) -> ParsedCommand {
        parse_command_from(
            std::iter::once("kimi").chain(arguments.iter().copied()),
            "1.2.3",
        )
        .expect("arguments")
    }

    #[tokio::test]
    async fn root_command_runs_the_existing_main_command_pipeline() {
        let runtime = RuntimeMock::default();
        let disposition = run_entrypoint(
            &runtime,
            &parse(&["--prompt", "hello"]),
            "1.2.3",
            &HashMap::new(),
        )
        .await
        .expect("entrypoint");

        assert_eq!(
            disposition,
            EntrypointDisposition::Completed {
                exit_code: 0,
                headless_completed: true,
            }
        );
        assert_eq!(
            runtime.calls.lock().expect("calls").as_slice(),
            [Call::Preflight(Some(false)), Call::Prompt]
        );
    }

    #[tokio::test]
    async fn subcommands_bypass_the_default_shell_and_preserve_exit_code() {
        let runtime = RuntimeMock::default();
        let disposition = run_entrypoint(&runtime, &parse(&["upgrade"]), "1.2.3", &HashMap::new())
            .await
            .expect("entrypoint");

        assert_eq!(
            disposition,
            EntrypointDisposition::Completed {
                exit_code: 7,
                headless_completed: false,
            }
        );
        assert_eq!(
            runtime.calls.lock().expect("calls").as_slice(),
            [Call::Subcommand("upgrade")]
        );
    }

    #[tokio::test]
    async fn option_conflicts_are_process_owned_exit_dispositions() {
        let runtime = RuntimeMock::default();
        let disposition = run_entrypoint(
            &runtime,
            &parse(&["--prompt", "hello", "--yolo"]),
            "1.2.3",
            &HashMap::new(),
        )
        .await
        .expect("entrypoint");

        assert_eq!(
            disposition,
            EntrypointDisposition::Exit {
                code: 1,
                stderr: Some("error: Cannot combine --prompt with --yolo.\n".to_owned()),
            }
        );
        assert!(runtime.calls.lock().expect("calls").is_empty());
    }
}
