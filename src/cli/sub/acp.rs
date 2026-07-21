use std::{collections::BTreeMap, error::Error, fmt};

use async_trait::async_trait;

use crate::{
    cli::version::{CLI_USER_AGENT_PRODUCT, HostIdentity},
    sdk::types::SkillSummary,
    tui::commands::skills::{SkillSlashCommands, build_skill_slash_commands},
};

pub const KIMI_CODE_HOME_ENV: &str = "KIMI_CODE_HOME";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessUiMode {
    Acp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpHarnessOptions {
    pub identity: HostIdentity,
    pub ui_mode: HarnessUiMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableCommandInput {
    pub hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableCommand {
    pub name: String,
    pub description: String,
    pub input: Option<AvailableCommandInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommandsSnapshot {
    pub commands: Vec<AvailableCommand>,
    pub skill_command_map: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpServerOptions {
    pub agent_info: AgentInfo,
    pub slash_commands: AcpSlashCommandResolver,
    pub terminal_auth_env: Option<BTreeMap<String, String>>,
    pub terminal_auth_legacy_command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpSlashCommandResolver {
    builtin_commands: Vec<AvailableCommand>,
}

impl Default for AcpSlashCommandResolver {
    fn default() -> Self {
        Self {
            builtin_commands: acp_builtin_slash_commands(),
        }
    }
}

impl AcpSlashCommandResolver {
    pub fn new(builtin_commands: Vec<AvailableCommand>) -> Self {
        Self { builtin_commands }
    }

    // Original:
    //   apps/kimi-code/src/cli/sub/acp.ts
    //   resolveSlashCommands()
    pub async fn resolve(&self, session: &dyn SkillListSession) -> SlashCommandsSnapshot {
        let skills = session.list_skills().await.unwrap_or_default();
        let SkillSlashCommands {
            commands,
            command_map,
        } = build_skill_slash_commands(&skills);
        let mut available = self.builtin_commands.clone();
        available.extend(commands.into_iter().map(|command| AvailableCommand {
            name: command.name,
            description: command.description,
            input: None,
        }));
        SlashCommandsSnapshot {
            commands: available,
            skill_command_map: command_map,
        }
    }

    pub fn builtin_commands(&self) -> &[AvailableCommand] {
        &self.builtin_commands
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpDisposition {
    Exit(i32),
}

#[derive(Debug)]
pub struct AcpRuntimeError(Box<dyn Error + Send + Sync>);

impl AcpRuntimeError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for AcpRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for AcpRuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

pub trait AcpHarness: Send {}

#[async_trait]
pub trait SkillListSession: Send + Sync {
    async fn list_skills(&self) -> Result<Vec<SkillSummary>, AcpRuntimeError>;
}

#[async_trait]
pub trait AcpRuntime: Send + Sync {
    async fn run_login_flow(&self) -> Result<(), AcpRuntimeError>;

    fn create_harness(
        &self,
        options: AcpHarnessOptions,
    ) -> Result<Box<dyn AcpHarness>, AcpRuntimeError>;

    async fn run_acp_server(
        &self,
        harness: Box<dyn AcpHarness>,
        options: AcpServerOptions,
    ) -> Result<(), AcpRuntimeError>;

    fn version(&self) -> &str;

    fn kimi_code_home(&self) -> Option<String>;

    fn legacy_command(&self) -> Option<String>;

    fn write_stderr(&self, text: &str);
}

// Original:
//   apps/kimi-code/src/cli/sub/acp.ts
//   ACP_BUILTIN_SLASH_COMMANDS projection
pub fn acp_builtin_slash_commands() -> Vec<AvailableCommand> {
    [
        (
            "compact",
            "Compact the conversation context",
            Some("<optional custom summarization instructions>"),
        ),
        ("status", "Show current session status", None),
        ("usage", "Show session token usage", None),
        ("mcp", "Show MCP server status", None),
        ("tasks", "List background tasks", None),
        ("help", "Show available ACP commands", None),
    ]
    .into_iter()
    .map(|(name, description, hint)| AvailableCommand {
        name: name.to_owned(),
        description: description.to_owned(),
        input: hint.map(|hint| AvailableCommandInput {
            hint: hint.to_owned(),
        }),
    })
    .collect()
}

// Original:
//   apps/kimi-code/src/cli/sub/acp.ts
//   registerAcpCommand().action()
//
// Rust adaptation:
//   Process termination is returned to the binary entrypoint. Harness and ACP
//   transport construction remain injected so the command can be parity-tested
//   without taking ownership of the test process's stdio.
pub async fn handle_acp(
    runtime: &dyn AcpRuntime,
    login: bool,
) -> Result<AcpDisposition, AcpRuntimeError> {
    if login {
        runtime.run_login_flow().await?;
        return Ok(AcpDisposition::Exit(0));
    }

    let version = runtime.version().to_owned();
    let harness = runtime.create_harness(AcpHarnessOptions {
        identity: HostIdentity {
            user_agent_product: CLI_USER_AGENT_PRODUCT.to_owned(),
            version: version.clone(),
        },
        ui_mode: HarnessUiMode::Acp,
    })?;
    let terminal_auth_env = runtime
        .kimi_code_home()
        .filter(|home| !home.is_empty())
        .map(|home| BTreeMap::from([(KIMI_CODE_HOME_ENV.to_owned(), home)]));
    let terminal_auth_legacy_command = runtime
        .legacy_command()
        .filter(|command| !command.is_empty());
    let options = AcpServerOptions {
        agent_info: AgentInfo {
            name: "Kimi Code CLI".to_owned(),
            version,
        },
        slash_commands: AcpSlashCommandResolver::default(),
        terminal_auth_env,
        terminal_auth_legacy_command,
    };

    match runtime.run_acp_server(harness, options).await {
        Ok(()) => Ok(AcpDisposition::Exit(0)),
        Err(error) => {
            runtime.write_stderr(&format!("acp server: fatal error: {error}\n"));
            Ok(AcpDisposition::Exit(1))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crate::sdk::types::SkillSource;

    use super::*;

    #[derive(Debug, Clone, Copy)]
    struct TestError(&'static str);

    impl fmt::Display for TestError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.0)
        }
    }

    impl Error for TestError {}

    struct HarnessMock;
    impl AcpHarness for HarnessMock {}

    struct RuntimeMock {
        version: String,
        home: Option<String>,
        legacy_command: Option<String>,
        login_calls: Mutex<usize>,
        harness_options: Mutex<Vec<AcpHarnessOptions>>,
        server_options: Mutex<Vec<AcpServerOptions>>,
        server_error: bool,
        stderr: Mutex<String>,
    }

    impl RuntimeMock {
        fn new() -> Self {
            Self {
                version: "1.2.3-test".to_owned(),
                home: None,
                legacy_command: Some("/opt/kimi/bin/kimi".to_owned()),
                login_calls: Mutex::new(0),
                harness_options: Mutex::new(Vec::new()),
                server_options: Mutex::new(Vec::new()),
                server_error: false,
                stderr: Mutex::new(String::new()),
            }
        }
    }

    #[async_trait]
    impl AcpRuntime for RuntimeMock {
        async fn run_login_flow(&self) -> Result<(), AcpRuntimeError> {
            *self.login_calls.lock().expect("login calls") += 1;
            Ok(())
        }

        fn create_harness(
            &self,
            options: AcpHarnessOptions,
        ) -> Result<Box<dyn AcpHarness>, AcpRuntimeError> {
            self.harness_options
                .lock()
                .expect("harness options")
                .push(options);
            Ok(Box::new(HarnessMock))
        }

        async fn run_acp_server(
            &self,
            _: Box<dyn AcpHarness>,
            options: AcpServerOptions,
        ) -> Result<(), AcpRuntimeError> {
            self.server_options
                .lock()
                .expect("server options")
                .push(options);
            if self.server_error {
                Err(AcpRuntimeError::new(TestError("stdio failed")))
            } else {
                Ok(())
            }
        }

        fn version(&self) -> &str {
            &self.version
        }

        fn kimi_code_home(&self) -> Option<String> {
            self.home.clone()
        }

        fn legacy_command(&self) -> Option<String> {
            self.legacy_command.clone()
        }

        fn write_stderr(&self, text: &str) {
            self.stderr.lock().expect("stderr").push_str(text);
        }
    }

    struct SessionMock {
        skills: Vec<SkillSummary>,
        fails: bool,
    }

    #[async_trait]
    impl SkillListSession for SessionMock {
        async fn list_skills(&self) -> Result<Vec<SkillSummary>, AcpRuntimeError> {
            if self.fails {
                Err(AcpRuntimeError::new(TestError("skill source failed")))
            } else {
                Ok(self.skills.clone())
            }
        }
    }

    fn skill(name: &str, source: SkillSource) -> SkillSummary {
        SkillSummary {
            name: name.to_owned(),
            description: Some(format!("{name} description")),
            path: Some(format!("/skills/{name}/SKILL.md")),
            source: Some(source),
            skill_type: Some("prompt".to_owned()),
            disable_model_invocation: None,
            is_sub_skill: None,
        }
    }

    #[tokio::test]
    async fn starts_acp_with_identity_agent_info_and_auth_forwarding() {
        let mut runtime = RuntimeMock::new();
        runtime.home = Some("/tmp/kimi-debug".to_owned());

        let disposition = handle_acp(&runtime, false).await.expect("acp");

        assert_eq!(disposition, AcpDisposition::Exit(0));
        assert_eq!(
            runtime.harness_options.lock().expect("harness").as_slice(),
            [AcpHarnessOptions {
                identity: HostIdentity {
                    user_agent_product: "kimi-code-cli".to_owned(),
                    version: "1.2.3-test".to_owned(),
                },
                ui_mode: HarnessUiMode::Acp,
            }]
        );
        let options = runtime.server_options.lock().expect("server options");
        assert_eq!(
            options[0].agent_info,
            AgentInfo {
                name: "Kimi Code CLI".to_owned(),
                version: "1.2.3-test".to_owned(),
            }
        );
        assert_eq!(
            options[0].terminal_auth_env,
            Some(BTreeMap::from([(
                "KIMI_CODE_HOME".to_owned(),
                "/tmp/kimi-debug".to_owned()
            )]))
        );
        assert_eq!(
            options[0].terminal_auth_legacy_command.as_deref(),
            Some("/opt/kimi/bin/kimi")
        );
    }

    #[tokio::test]
    async fn omits_empty_auth_environment_and_legacy_command() {
        let mut runtime = RuntimeMock::new();
        runtime.home = Some(String::new());
        runtime.legacy_command = Some(String::new());

        handle_acp(&runtime, false).await.expect("acp");

        let options = runtime.server_options.lock().expect("server options");
        assert_eq!(options[0].terminal_auth_env, None);
        assert_eq!(options[0].terminal_auth_legacy_command, None);
    }

    #[tokio::test]
    async fn login_exits_zero_without_constructing_or_starting_server() {
        let runtime = RuntimeMock::new();

        let disposition = handle_acp(&runtime, true).await.expect("login");

        assert_eq!(disposition, AcpDisposition::Exit(0));
        assert_eq!(*runtime.login_calls.lock().expect("login calls"), 1);
        assert!(runtime.harness_options.lock().expect("harness").is_empty());
        assert!(runtime.server_options.lock().expect("server").is_empty());
    }

    #[tokio::test]
    async fn server_failure_reports_fatal_error_and_exits_one() {
        let mut runtime = RuntimeMock::new();
        runtime.server_error = true;

        let disposition = handle_acp(&runtime, false).await.expect("handled error");

        assert_eq!(disposition, AcpDisposition::Exit(1));
        assert_eq!(
            runtime.stderr.lock().expect("stderr").as_str(),
            "acp server: fatal error: stdio failed\n"
        );
    }

    #[tokio::test]
    async fn resolves_builtins_and_session_skills_from_one_snapshot() {
        let resolver = AcpSlashCommandResolver::default();
        let snapshot = resolver
            .resolve(&SessionMock {
                skills: vec![
                    skill("review", SkillSource::Project),
                    skill("mcp-config", SkillSource::Builtin),
                ],
                fails: false,
            })
            .await;

        assert_eq!(
            snapshot
                .commands
                .iter()
                .map(|command| command.name.as_str())
                .collect::<Vec<_>>(),
            [
                "compact",
                "status",
                "usage",
                "mcp",
                "tasks",
                "help",
                "mcp-config",
                "skill:review"
            ]
        );
        assert_eq!(
            snapshot.skill_command_map,
            [
                ("mcp-config".to_owned(), "mcp-config".to_owned()),
                ("skill:review".to_owned(), "review".to_owned())
            ]
        );
    }

    #[tokio::test]
    async fn skill_listing_failure_degrades_to_builtins_only() {
        let resolver = AcpSlashCommandResolver::default();
        let snapshot = resolver
            .resolve(&SessionMock {
                skills: Vec::new(),
                fails: true,
            })
            .await;

        assert_eq!(snapshot.commands, acp_builtin_slash_commands());
        assert!(snapshot.skill_command_map.is_empty());
    }
}
