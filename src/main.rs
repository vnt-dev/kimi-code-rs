use std::{
    collections::HashMap,
    env,
    error::Error,
    fmt,
    io::{self, Write},
    process::ExitCode,
    sync::Arc,
};

use async_trait::async_trait;
use kimi_code_rs::cli::{
    build_info::KIMI_BUILD_INFO,
    commands::{
        CommandInvocation, DoctorArgs, DoctorCommand, ProviderCommand, ServerArgs,
        parse_command_from,
    },
    entrypoint::{
        EntrypointDisposition, EntrypointRuntime, EntrypointRuntimeError, SubcommandOutcome,
        run_entrypoint,
    },
    main_command::{MainCommandRuntime, MainCommandRuntimeError},
    options::CliOptions,
    prompt_runtime::{ProcessPromptOutput, SystemPromptRuntime},
    startup_error::{StartupErrorFormatOptions, StartupFailure, format_startup_error},
    sub::{
        doctor::{DoctorOptions, DoctorTarget, handle_doctor},
        doctor_runtime::SystemDoctorRuntime,
        provider::{KIMI_REGISTRY_API_KEY_ENV, run_provider_command},
        provider_runtime::ProviderCommandRuntime,
        upgrade::{UpgradeError, handle_upgrade},
        upgrade_runtime::{SystemUpgradeRuntime, UpgradeObserver},
        web::deprecated_server::{
            DeprecatedServerDisposition, DeprecatedServerRuntime, handle_deprecated_server,
        },
    },
    update::types::UpdatePreflightResult,
    version::{create_kimi_code_user_agent, get_host_package_root, get_version},
};
use kimi_code_rs::utils::paths::get_data_dir;
use serde_json::{Map, Value};

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

#[derive(Debug, Clone, Copy)]
struct PendingTelemetryObserver;

impl UpgradeObserver for PendingTelemetryObserver {
    fn track(&self, _: &str, _: &Map<String, Value>) -> Result<(), UpgradeError> {
        Ok(())
    }

    fn log_info(&self, _: &str, _: &Map<String, Value>) -> Result<(), UpgradeError> {
        Ok(())
    }

    fn log_warn(&self, _: &str, _: &Map<String, Value>) -> Result<(), UpgradeError> {
        Ok(())
    }
}

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

    async fn run_prompt(
        &self,
        options: &CliOptions,
        version: &str,
    ) -> Result<(), MainCommandRuntimeError> {
        let data_dir = get_data_dir().map_err(MainCommandRuntimeError::new)?;
        let runtime = SystemPromptRuntime::new(data_dir, version, env::vars().collect())
            .map_err(MainCommandRuntimeError::new)?;
        runtime
            .run(
                options,
                &mut ProcessPromptOutput::stdout(),
                &mut ProcessPromptOutput::stderr(),
            )
            .await
            .map_err(MainCommandRuntimeError::new)
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
        version: &str,
    ) -> Result<SubcommandOutcome, EntrypointRuntimeError> {
        if let CommandInvocation::Provider(arguments) = command {
            let data_dir = get_data_dir().map_err(EntrypointRuntimeError::new)?;
            let environment_api_key = env::var(KIMI_REGISTRY_API_KEY_ENV).ok();
            return run_provider_subcommand(
                &arguments.command,
                version,
                &data_dir,
                environment_api_key.as_deref(),
            )
            .await;
        }
        if matches!(command, CommandInvocation::Upgrade) {
            return run_upgrade_subcommand(version).await;
        }
        if let CommandInvocation::Doctor(arguments) = command {
            return run_doctor_subcommand(arguments).await;
        }
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

// Original:
//   apps/kimi-code/src/cli/sub/doctor.ts
//   registerDoctorCommand(), resolveDeps()
async fn run_doctor_subcommand(
    arguments: &DoctorArgs,
) -> Result<SubcommandOutcome, EntrypointRuntimeError> {
    let options = match &arguments.command {
        None => DoctorOptions::default(),
        Some(DoctorCommand::Config { path }) => DoctorOptions {
            target: Some(DoctorTarget::Config),
            path: path.as_deref().map(Into::into),
        },
        Some(DoctorCommand::Tui { path }) => DoctorOptions {
            target: Some(DoctorTarget::Tui),
            path: path.as_deref().map(Into::into),
        },
    };
    let runtime = SystemDoctorRuntime::new().map_err(EntrypointRuntimeError::new)?;
    let exit_code = handle_doctor(&runtime, &options)
        .await
        .map_err(EntrypointRuntimeError::new)?;
    Ok(SubcommandOutcome { exit_code })
}

// Original:
//   apps/kimi-code/src/main.ts
//   handleUpgradeCommand()
//
// Rust adaptation:
//   The migrated system upgrade runtime owns CDN I/O, prompt interaction, and
//   installer process lifecycle. A compile-time build target replaces SEA
//   detection for native Rust distributions.
async fn run_upgrade_subcommand(
    version: &str,
) -> Result<SubcommandOutcome, EntrypointRuntimeError> {
    // MIGRATION-TODO:
    // Original: handleUpgradeCommand() initializes telemetry and diagnostic
    // logging before running the upgrade.
    // Temporary behavior: upgrade telemetry and logs are discarded.
    // Completion condition: compose the process telemetry observer here.
    let runtime = SystemUpgradeRuntime::new(
        get_host_package_root(),
        KIMI_BUILD_INFO.build_target.is_some(),
        Arc::new(PendingTelemetryObserver),
    );
    let exit_code = handle_upgrade(&runtime, version)
        .await
        .map_err(EntrypointRuntimeError::new)?;
    Ok(SubcommandOutcome { exit_code })
}

// Original:
//   apps/kimi-code/src/cli/sub/provider.ts
//   resolveDeps(), registerProviderCommand().action()
//
// Rust adaptation:
//   The SDK harness dependency is replaced by the migrated config store and
//   HTTP runtime. The async handler and its config-write ordering are unchanged.
async fn run_provider_subcommand(
    command: &ProviderCommand,
    version: &str,
    data_dir: &std::path::Path,
    environment_api_key: Option<&str>,
) -> Result<SubcommandOutcome, EntrypointRuntimeError> {
    let user_agent = create_kimi_code_user_agent(version).map_err(EntrypointRuntimeError::new)?;
    let runtime = ProviderCommandRuntime::new(data_dir.join("config.toml"), user_agent);
    let exit_code = run_provider_command(&runtime, command, environment_api_key).await;
    Ok(SubcommandOutcome { exit_code })
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
    use uuid::Uuid;

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

    #[tokio::test]
    async fn provider_list_uses_the_migrated_config_runtime() {
        let data_dir = env::temp_dir().join(format!("kimi-entry-provider-{}", Uuid::new_v4()));
        let outcome = run_provider_subcommand(
            &ProviderCommand::List { json: false },
            "1.2.3",
            &data_dir,
            None,
        )
        .await
        .expect("provider list");

        assert_eq!(outcome, SubcommandOutcome { exit_code: 0 });
        let config = tokio::fs::read_to_string(data_dir.join("config.toml"))
            .await
            .expect("created config");
        assert!(config.contains("built-in defaults can apply"));
        tokio::fs::remove_dir_all(&data_dir)
            .await
            .expect("remove provider test directory");
    }
}
