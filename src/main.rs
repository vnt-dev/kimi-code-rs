use std::{
    collections::HashMap,
    env,
    error::Error,
    fmt,
    io::{self, Write},
    process::ExitCode,
};

use async_trait::async_trait;
use kimi_code_rs::cli::{
    commands::{CommandInvocation, ServerArgs, parse_command_from},
    entrypoint::{
        EntrypointDisposition, EntrypointRuntime, EntrypointRuntimeError, SubcommandOutcome,
        run_entrypoint,
    },
    main_command::{MainCommandRuntime, MainCommandRuntimeError},
    options::CliOptions,
    startup_error::{StartupErrorFormatOptions, StartupFailure, format_startup_error},
    sub::web::deprecated_server::{
        DeprecatedServerDisposition, DeprecatedServerRuntime, handle_deprecated_server,
    },
    update::types::UpdatePreflightResult,
    version::get_version,
};

#[derive(Debug)]
struct MigrationPending {
    original: &'static str,
    completion: &'static str,
}

impl fmt::Display for MigrationPending {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "migration pending for {}; completion condition: {}",
            self.original, self.completion
        )
    }
}

impl Error for MigrationPending {}

struct SystemEntrypointRuntime;

impl DeprecatedServerRuntime for SystemEntrypointRuntime {
    fn write_stderr(&self, text: &str) {
        eprint!("{text}");
    }
}

#[async_trait]
impl MainCommandRuntime for SystemEntrypointRuntime {
    async fn run_update_preflight(
        &self,
        _: &str,
        _: Option<bool>,
    ) -> Result<UpdatePreflightResult, MainCommandRuntimeError> {
        // MIGRATION-TODO:
        // Original: src/main.ts, runUpdatePreflight().
        // Missing dependency: the process entrypoint cannot yet compose the
        // SDK-backed update observer and telemetry client.
        // Temporary behavior: continue to the selected UI runner.
        // Completion condition: construct SystemUpdatePreflightRuntime here.
        Ok(UpdatePreflightResult::Continue)
    }

    async fn run_prompt(&self, _: &CliOptions, _: &str) -> Result<(), MainCommandRuntimeError> {
        Err(MainCommandRuntimeError::new(MigrationPending {
            original: "src/main.ts runPrompt()",
            completion: "compose a concrete PromptRuntime and PromptSessionFactory",
        }))
    }

    async fn run_shell(&self, _: &CliOptions, _: &str) -> Result<(), MainCommandRuntimeError> {
        Err(MainCommandRuntimeError::new(MigrationPending {
            original: "src/main.ts runShell()",
            completion: "port the SDK harness and KimiTUI coordinator runtime",
        }))
    }
}

#[async_trait]
impl EntrypointRuntime for SystemEntrypointRuntime {
    async fn run_subcommand(
        &self,
        command: &CommandInvocation,
        _: &str,
    ) -> Result<SubcommandOutcome, EntrypointRuntimeError> {
        if let CommandInvocation::Server(arguments) = command
            && !is_legacy_kill(arguments)
        {
            let DeprecatedServerDisposition::Exit(exit_code) = handle_deprecated_server(self);
            return Ok(SubcommandOutcome { exit_code });
        }
        if let CommandInvocation::Server(arguments) = command
            && is_legacy_kill(arguments)
        {
            return Err(EntrypointRuntimeError::new(MigrationPending {
                original: "src/cli/sub/web/legacy-kill.ts handleLegacyKillCommand()",
                completion: "compose lock-file I/O, shutdown HTTP, and safe process signaling",
            }));
        }

        Err(EntrypointRuntimeError::new(MigrationPending {
            original: subcommand_source(command),
            completion: "compose the migrated handler with its concrete process runtime",
        }))
    }
}

fn is_legacy_kill(arguments: &ServerArgs) -> bool {
    arguments
        .legacy_args
        .first()
        .is_some_and(|argument| argument == "kill")
}

fn subcommand_source(command: &CommandInvocation) -> &'static str {
    match command {
        CommandInvocation::Export(_) => "src/cli/commands.ts export action",
        CommandInvocation::Provider(_) => "src/cli/commands.ts provider action",
        CommandInvocation::Acp(_) => "src/cli/commands.ts acp action",
        CommandInvocation::Web(_) => "src/cli/commands.ts web action",
        CommandInvocation::Server(_) => "src/cli/commands.ts server action",
        CommandInvocation::Login => "src/cli/commands.ts login action",
        CommandInvocation::Doctor(_) => "src/cli/commands.ts doctor action",
        CommandInvocation::Vis(_) => "src/cli/commands.ts vis action",
        CommandInvocation::Migrate => "src/main.ts handleMigrateCommand()",
        CommandInvocation::Upgrade => "src/main.ts handleUpgradeCommand()",
        CommandInvocation::PluginRunNode(_) => "src/main.ts runPluginNodeEntry()",
    }
}

// Original:
//   apps/kimi-code/src/main.ts
//   main()
//
// Rust adaptation:
//   Rust's async main owns argument parsing, stderr, and the final ExitCode.
//   Unlike the JavaScript callback graph, every selected action is awaited.
#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    // MIGRATION-TODO:
    // Original: main.ts installs crash handlers, the global proxy dispatcher,
    // native module hooks/smoke checks, and schedules native-cache cleanup.
    // Missing dependency: the native asset and telemetry process adapters.
    // Completion condition: install them here before parsing arguments.
    let version = get_version();
    let parsed = match parse_command_from(env::args_os(), version) {
        Ok(parsed) => parsed,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return process_exit_code(code);
        }
    };
    let environment = env::vars().collect::<HashMap<_, _>>();

    match run_entrypoint(&SystemEntrypointRuntime, &parsed, version, &environment).await {
        Ok(EntrypointDisposition::Completed {
            exit_code,
            headless_completed,
        }) => {
            if headless_completed {
                let _ = io::stdout().flush();
                let _ = io::stderr().flush();
            }
            process_exit_code(exit_code)
        }
        Ok(EntrypointDisposition::Exit { code, stderr }) => {
            if let Some(stderr) = stderr {
                eprint!("{stderr}");
            }
            process_exit_code(code)
        }
        Err(error) => {
            eprint!(
                "{}",
                format_startup_error(
                    StartupFailure::Other(&error),
                    &StartupErrorFormatOptions {
                        operation: Some(error.operation()),
                        ..StartupErrorFormatOptions::default()
                    },
                )
            );
            // MIGRATION-TODO:
            // Original: logStartupFailure() flushes diagnostic logs and prints
            // resolveGlobalLogPath(resolveKimiHome()).
            // Completion condition: port the SDK diagnostic logger boundary.
            process_exit_code(1)
        }
    }
}

fn process_exit_code(code: i32) -> ExitCode {
    match u8::try_from(code) {
        Ok(code) => ExitCode::from(code),
        Err(_) => ExitCode::FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(arguments: &[&str]) -> CommandInvocation {
        CommandInvocation::Server(ServerArgs {
            legacy_args: arguments
                .iter()
                .map(|argument| (*argument).to_owned())
                .collect(),
        })
    }

    #[test]
    fn recognizes_only_the_registered_legacy_kill_subcommand() {
        for arguments in [
            vec!["kill"],
            vec!["kill", "old-id"],
            vec!["kill", "--force"],
        ] {
            let CommandInvocation::Server(arguments) = server(&arguments) else {
                unreachable!()
            };
            assert!(is_legacy_kill(&arguments));
        }

        for arguments in [vec![], vec!["run"], vec!["Kill"], vec!["--port", "1234"]] {
            let CommandInvocation::Server(arguments) = server(&arguments) else {
                unreachable!()
            };
            assert!(!is_legacy_kill(&arguments));
        }
    }

    #[tokio::test]
    async fn system_runtime_executes_the_deprecated_server_shim() {
        let outcome = SystemEntrypointRuntime
            .run_subcommand(&server(&["run", "--port", "1234"]), "1.2.3")
            .await
            .expect("deprecated server disposition");

        assert_eq!(outcome, SubcommandOutcome { exit_code: 1 });
    }

    #[tokio::test]
    async fn legacy_kill_remains_an_explicit_pending_runtime_boundary() {
        let error = SystemEntrypointRuntime
            .run_subcommand(&server(&["kill"]), "1.2.3")
            .await
            .expect_err("legacy kill runtime");

        assert!(error.to_string().contains("handleLegacyKillCommand"));
        assert!(error.to_string().contains("safe process signaling"));
    }
}
