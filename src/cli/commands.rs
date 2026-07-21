use std::ffi::OsString;

use clap::{ArgAction, Args, CommandFactory, FromArgMatches, Parser, Subcommand};

use super::options::{CliOptions, PromptOutputFormat};
use super::sub::web::shared::{DEFAULT_LAN_HOST, DEFAULT_SERVER_PORT};

pub const CLI_COMMAND_NAME: &str = "kimi";

#[derive(Debug, Parser)]
#[command(
    name = CLI_COMMAND_NAME,
    about = "The Starting Point for Next-Gen Agents",
    help_template = "{before-help}{name} {version}\n{about}\n\nUsage: {usage}\n\n{all-args}{after-help}",
    after_help = "Documentation:        https://moonshotai.github.io/kimi-code/"
)]
struct RootCommand {
    /// Resume a session. With ID: resume it; without ID: interactively pick.
    #[arg(
        short = 'S',
        short_alias = 'r',
        long,
        alias = "resume",
        num_args = 0..=1,
        default_missing_value = ""
    )]
    session: Option<String>,

    /// Continue the previous session for the working directory.
    #[arg(short = 'c', short_alias = 'C', long = "continue")]
    continue_previous: bool,

    /// Auto-approve regular tool calls; the agent may still ask questions.
    #[arg(short = 'y', long, aliases = ["yes", "auto-approve"])]
    yolo: bool,

    /// Start in fully autonomous permission mode.
    #[arg(long)]
    auto: bool,

    /// Start in plan mode.
    #[arg(long)]
    plan: bool,

    /// LLM model alias for this invocation.
    #[arg(short = 'm', long)]
    model: Option<String>,

    /// Run one prompt non-interactively and print the response.
    #[arg(short = 'p', long)]
    prompt: Option<String>,

    /// Output format for prompt mode.
    #[arg(long, value_parser = ["text", "stream-json"])]
    output_format: Option<String>,

    /// Load skills from this directory. Can be repeated.
    #[arg(long = "skills-dir", action = ArgAction::Append)]
    skills_dirs: Vec<String>,

    /// Agent profile to use for this invocation.
    #[arg(long, conflicts_with = "agent_file")]
    agent: Option<String>,

    /// Load and select an agent definition from a Markdown file.
    #[arg(long = "agent-file", conflicts_with = "agent")]
    agent_file: Option<String>,

    /// Add an additional workspace directory. Can be repeated.
    #[arg(long = "add-dir", action = ArgAction::Append)]
    add_dirs: Vec<String>,

    #[command(subcommand)]
    command: Option<CommandInvocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum CommandInvocation {
    /// Export a session as a ZIP archive.
    Export(ExportArgs),
    /// Manage LLM providers non-interactively.
    Provider(ProviderArgs),
    /// Run as an Agent Client Protocol server over stdio.
    Acp(AcpArgs),
    /// Run the local Kimi server and open the web UI.
    Web(WebArgs),
    /// Deprecated server command shim.
    Server(ServerArgs),
    /// Authenticate through the device-code flow.
    Login,
    /// Validate Kimi Code configuration files.
    Doctor(DoctorArgs),
    /// Launch the session visualizer.
    Vis(VisArgs),
    /// Migrate data from a legacy kimi-cli installation.
    Migrate,
    /// Upgrade Kimi Code to the latest version.
    #[command(alias = "update")]
    Upgrade,
    #[command(name = "__plugin_run_node", hide = true)]
    PluginRunNode(PluginRunNodeArgs),
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ExportArgs {
    #[arg(short = 'o', long)]
    pub output: Option<String>,
    #[arg(short = 'y', long)]
    pub yes: bool,
    #[arg(
        long = "no-include-global-log",
        action = ArgAction::SetFalse,
        default_value_t = true
    )]
    pub include_global_log: bool,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ProviderArgs {
    #[command(subcommand)]
    pub command: ProviderCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum ProviderCommand {
    Add {
        url: String,
        #[arg(long)]
        api_key: Option<String>,
    },
    Remove {
        provider_id: String,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum CatalogCommand {
    List {
        provider_id: Option<String>,
        #[arg(long)]
        filter: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Add {
        provider_id: String,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        default_model: Option<String>,
        #[arg(long)]
        url: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct AcpArgs {
    #[arg(long)]
    pub login: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct WebArgs {
    #[arg(long, default_value = DEFAULT_SERVER_PORT)]
    pub port: String,
    #[arg(long, num_args = 0..=1, default_missing_value = DEFAULT_LAN_HOST)]
    pub host: Option<String>,
    #[arg(long = "allowed-host", num_args = 1.., action = ArgAction::Append)]
    pub allowed_hosts: Vec<String>,
    #[arg(long, default_value_t = true)]
    pub insecure_no_tls: bool,
    #[arg(long)]
    pub allow_remote_shutdown: bool,
    #[arg(long)]
    pub allow_remote_terminals: bool,
    #[arg(long)]
    pub dangerous_bypass_auth: bool,
    #[arg(
        long,
        value_parser = ["fatal", "error", "warn", "info", "debug", "trace", "silent"]
    )]
    pub log_level: Option<String>,
    #[arg(long)]
    pub debug_endpoints: bool,
    #[arg(long = "no-open", action = ArgAction::SetFalse, default_value_t = true)]
    pub open: bool,
    #[command(subcommand)]
    pub command: Option<WebCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum WebCommand {
    RotateToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
#[command(trailing_var_arg = true)]
pub struct ServerArgs {
    /// Legacy arguments are intentionally swallowed by the deprecation shim.
    #[arg(allow_hyphen_values = true)]
    pub legacy_args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct DoctorArgs {
    #[command(subcommand)]
    pub command: Option<DoctorCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum DoctorCommand {
    Config { path: Option<String> },
    Tui { path: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct VisArgs {
    #[arg(long)]
    pub port: Option<String>,
    #[arg(long)]
    pub host: Option<String>,
    #[arg(long = "no-open", action = ArgAction::SetFalse, default_value_t = true)]
    pub open: bool,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
#[command(trailing_var_arg = true)]
pub struct PluginRunNodeArgs {
    pub entry: String,
    #[arg(allow_hyphen_values = true)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    pub options: CliOptions,
    pub command: Option<CommandInvocation>,
}

// Original:
//   apps/kimi-code/src/cli/commands.ts
//   createProgram()
//
// Rust adaptation:
//   Parsing returns a typed command value. Execution remains in the entrypoint
//   and subcommand handlers, matching Commander's parse/action separation.
pub fn parse_command_from<I, T>(arguments: I, version: &str) -> Result<ParsedCommand, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let leaked_version: &'static str = Box::leak(version.to_owned().into_boxed_str());
    let matches = RootCommand::command()
        .version(leaked_version)
        .try_get_matches_from(arguments)?;
    let root = RootCommand::from_arg_matches(&matches)?;
    let output_format = match root.output_format.as_deref() {
        Some("stream-json") => Some(PromptOutputFormat::StreamJson),
        Some("text") => Some(PromptOutputFormat::Text),
        None => None,
        Some(_) => unreachable!("clap validates output formats"),
    };

    Ok(ParsedCommand {
        options: CliOptions {
            session: root.session,
            continue_previous: root.continue_previous,
            yolo: root.yolo,
            auto: root.auto,
            plan: root.plan,
            model: root.model,
            output_format,
            prompt: root.prompt,
            skills_dirs: root.skills_dirs,
            agent: root.agent,
            agent_files: root.agent_file.into_iter().collect(),
            add_dirs: root.add_dirs,
        },
        command: root.command,
    })
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, error::ErrorKind};

    use super::{
        CatalogCommand, CommandInvocation, DoctorCommand, ProviderCommand, RootCommand, WebCommand,
        parse_command_from,
    };
    use crate::cli::options::PromptOutputFormat;

    fn parse(arguments: &[&str]) -> super::ParsedCommand {
        parse_command_from(
            std::iter::once("kimi").chain(arguments.iter().copied()),
            "0.1.0-test",
        )
        .expect("arguments should parse")
    }

    #[test]
    fn parses_root_defaults_and_repeated_directories() {
        let parsed = parse(&[]);
        assert_eq!(parsed.options, Default::default());
        assert_eq!(parsed.command, None);

        let options = parse(&[
            "--skills-dir",
            "/one",
            "--skills-dir=/two",
            "--add-dir",
            "/shared",
        ])
        .options;
        assert_eq!(options.skills_dirs, ["/one", "/two"]);
        assert_eq!(options.add_dirs, ["/shared"]);
    }

    #[test]
    fn parses_session_aliases_and_picker_forms() {
        for flag in ["--session", "-S", "-r", "--resume"] {
            assert_eq!(parse(&[flag]).options.session.as_deref(), Some(""));
        }
        assert_eq!(
            parse(&["--session", "ses-123"]).options.session.as_deref(),
            Some("ses-123")
        );
        assert!(parse(&["-C"]).options.continue_previous);
    }

    #[test]
    fn parses_permission_model_and_prompt_options() {
        for flag in ["--yolo", "-y", "--yes", "--auto-approve"] {
            assert!(parse(&[flag]).options.yolo);
        }
        let options = parse(&[
            "--auto",
            "--plan",
            "-m",
            "kimi-code/k2",
            "-p",
            "run",
            "--output-format=stream-json",
        ])
        .options;
        assert!(options.auto && options.plan);
        assert_eq!(options.model.as_deref(), Some("kimi-code/k2"));
        assert_eq!(options.prompt.as_deref(), Some("run"));
        assert_eq!(options.output_format, Some(PromptOutputFormat::StreamJson));
    }

    #[test]
    fn rejects_repeated_or_mixed_agent_selectors() {
        for arguments in [
            &["--agent", "one", "--agent", "two"][..],
            &["--agent-file", "a.md", "--agent-file", "b.md"][..],
            &["--agent", "one", "--agent-file", "a.md"][..],
        ] {
            assert!(
                parse_command_from(
                    std::iter::once("kimi").chain(arguments.iter().copied()),
                    "x"
                )
                .is_err()
            );
        }
    }

    #[test]
    fn routes_provider_catalog_commands() {
        let parsed = parse(&[
            "provider",
            "catalog",
            "add",
            "openai",
            "--api-key",
            "secret",
            "--default-model",
            "gpt",
        ]);
        assert!(matches!(
            parsed.command,
            Some(CommandInvocation::Provider(super::ProviderArgs {
                command: ProviderCommand::Catalog {
                    command: CatalogCommand::Add {
                        provider_id,
                        api_key: Some(_),
                        default_model: Some(_),
                        ..
                    }
                }
            })) if provider_id == "openai"
        ));
    }

    #[test]
    fn parses_web_defaults_optional_host_and_rotate_token() {
        let parsed = parse(&["web"]);
        let Some(CommandInvocation::Web(web)) = parsed.command else {
            panic!("web command");
        };
        assert_eq!(web.port, "58627");
        assert_eq!(web.host, None);
        assert!(web.open && web.insecure_no_tls);

        let parsed = parse(&["web", "--host", "--no-open", "rotate-token"]);
        let Some(CommandInvocation::Web(web)) = parsed.command else {
            panic!("web command");
        };
        assert_eq!(web.host.as_deref(), Some("0.0.0.0"));
        assert!(!web.open);
        assert_eq!(web.command, Some(WebCommand::RotateToken));
    }

    #[test]
    fn routes_doctor_export_vis_acp_and_hidden_plugin_commands() {
        assert!(matches!(
            parse(&["doctor", "config", "custom.toml"]).command,
            Some(CommandInvocation::Doctor(super::DoctorArgs {
                command: Some(DoctorCommand::Config { path: Some(_) })
            }))
        ));
        assert!(matches!(
            parse(&["export", "ses-1"]).command,
            Some(CommandInvocation::Export(_))
        ));
        assert!(matches!(
            parse(&["vis", "--no-open", "ses-1"]).command,
            Some(CommandInvocation::Vis(_))
        ));
        assert!(matches!(
            parse(&["acp", "--login"]).command,
            Some(CommandInvocation::Acp(_))
        ));
        assert!(matches!(
            parse(&["__plugin_run_node", "tool.mjs", "--", "--flag"]).command,
            Some(CommandInvocation::PluginRunNode(_))
        ));
    }

    #[test]
    fn preserves_upgrade_alias_and_deprecated_server_passthrough() {
        assert_eq!(parse(&["update"]).command, Some(CommandInvocation::Upgrade));
        let Some(CommandInvocation::Server(server)) =
            parse(&["server", "run", "--port", "12"]).command
        else {
            panic!("server command");
        };
        assert_eq!(server.legacy_args, ["run", "--port", "12"]);
    }

    #[test]
    fn exposes_the_original_visible_command_order() {
        let command = RootCommand::command();
        let names: Vec<_> = command
            .get_subcommands()
            .filter(|command| !command.is_hide_set())
            .map(|command| command.get_name())
            .collect();
        assert_eq!(
            names,
            [
                "export", "provider", "acp", "web", "server", "login", "doctor", "vis", "migrate",
                "upgrade"
            ]
        );
    }

    #[test]
    fn reports_version_and_rejects_removed_flags() {
        let error = parse_command_from(["kimi", "--version"], "1.2.3")
            .expect_err("version exits through clap");
        assert_eq!(error.kind(), ErrorKind::DisplayVersion);
        assert!(error.to_string().contains("1.2.3"));

        for flag in ["--verbose", "--debug", "--work-dir=/", "--print", "--wire"] {
            assert!(parse_command_from(["kimi", flag], "x").is_err(), "{flag}");
        }
    }
}
