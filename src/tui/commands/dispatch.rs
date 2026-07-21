use std::collections::BTreeMap;

use async_trait::async_trait;
use indexmap::IndexMap;

use super::{
    parse::parse_slash_input,
    resolve::{
        ResolveSlashCommandInput, SlashCommandIntent, resolve_slash_command_input,
        slash_busy_message,
    },
    types::ParsedSlashInput,
};

pub const LLM_NOT_SET_MESSAGE: &str = "No model configured. Run /login to sign in, then try again.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchState {
    pub model: String,
    pub has_session: bool,
    pub is_streaming: bool,
    pub is_compacting: bool,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchTelemetryEvent {
    pub name: String,
    pub properties: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltinCommandInvocation {
    pub name: &'static str,
    pub args: String,
}

#[async_trait(?Send)]
pub trait SlashCommandDispatchHost {
    fn dispatch_state(&self) -> DispatchState;
    fn skill_command_map(&self) -> &IndexMap<String, String>;
    fn plugin_command_map(&self) -> &IndexMap<String, String>;
    fn track_dispatch(&mut self, event: DispatchTelemetryEvent);
    fn show_error(&mut self, message: &str);
    fn show_status(&mut self, message: &str);
    fn send_normal_user_input(&mut self, text: &str);
    fn send_skill_activation(&mut self, skill_name: &str, args: &str);
    fn activate_plugin_command(&mut self, plugin_id: &str, command_name: &str, args: &str);
    fn try_handle_dance_command(&mut self, parsed: &ParsedSlashInput) -> bool;

    async fn stop(&mut self) -> Result<(), String>;
    fn show_help_panel(&mut self);
    async fn create_new_session(&mut self) -> Result<(), String>;
    fn request_render(&mut self);
    async fn show_session_picker(&mut self) -> Result<(), String>;
    async fn show_tasks_browser(&mut self) -> Result<(), String>;
    async fn run_builtin_command(
        &mut self,
        invocation: BuiltinCommandInvocation,
    ) -> Result<(), String>;
}

fn telemetry(
    name: &str,
    properties: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
) -> DispatchTelemetryEvent {
    DispatchTelemetryEvent {
        name: name.to_owned(),
        properties: properties
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect(),
    }
}

// Original: `src/tui/commands/dispatch.ts`, `dispatchInput()`.
//
// Rust adaptation: callers await or spawn this future explicitly instead of
// relying on JavaScript's discarded Promise.
pub async fn dispatch_input(host: &mut impl SlashCommandDispatchHost, text: &str) {
    if parse_slash_input(text).is_some() {
        execute_slash_command(host, text).await;
    } else {
        host.send_normal_user_input(text);
    }
}

// Original: `executeSlashCommand()`.
pub async fn execute_slash_command(host: &mut impl SlashCommandDispatchHost, input: &str) {
    let parsed = parse_slash_input(input);
    let state = host.dispatch_state();
    let intent = resolve_slash_command_input(ResolveSlashCommandInput {
        input,
        skill_command_map: host.skill_command_map(),
        plugin_command_map: host.plugin_command_map(),
        is_streaming: state.is_streaming,
        is_compacting: state.is_compacting,
    });

    match intent {
        SlashCommandIntent::NotCommand => {}
        SlashCommandIntent::Blocked {
            command_name,
            reason,
        } => {
            host.track_dispatch(telemetry(
                "input_command_invalid",
                [
                    ("reason", "blocked".to_owned()),
                    ("command", command_name.clone()),
                ],
            ));
            host.show_error(&slash_busy_message(&command_name, reason));
        }
        SlashCommandIntent::Skill {
            command_name,
            skill_name,
            args,
        } => {
            if state.model.trim().is_empty() || !state.has_session {
                host.show_error(LLM_NOT_SET_MESSAGE);
                return;
            }
            host.track_dispatch(telemetry(
                "input_command",
                [
                    ("command", command_name),
                    ("skill_name", skill_name.clone()),
                ],
            ));
            host.send_skill_activation(&skill_name, &args);
        }
        SlashCommandIntent::PluginCommand {
            command_name,
            plugin_id,
            args,
        } => {
            if state.model.trim().is_empty() || !state.has_session {
                host.show_error(LLM_NOT_SET_MESSAGE);
                return;
            }
            host.track_dispatch(telemetry(
                "input_command",
                [("command", format!("{plugin_id}:{command_name}"))],
            ));
            host.activate_plugin_command(&plugin_id, &command_name, &args);
        }
        SlashCommandIntent::Message { input } => {
            if parsed
                .as_ref()
                .is_some_and(|parsed| host.try_handle_dance_command(parsed))
            {
                return;
            }
            host.send_normal_user_input(&input);
        }
        SlashCommandIntent::Builtin { name, args, .. } => {
            host.track_dispatch(telemetry("input_command", [("command", name)]));
            if name == "new" && parsed.as_ref().is_some_and(|parsed| parsed.name == "clear") {
                host.track_dispatch(telemetry("clear", std::iter::empty::<(&str, &str)>()));
            }
            if let Err(error) = handle_built_in_slash_command(host, name, args).await {
                host.show_error(&error);
            }
        }
    }
}

// Original: `handleBuiltInSlashCommand()`.
pub async fn handle_built_in_slash_command(
    host: &mut impl SlashCommandDispatchHost,
    name: &'static str,
    args: String,
) -> Result<(), String> {
    match name {
        "exit" => host.stop().await,
        "help" => {
            host.show_help_panel();
            Ok(())
        }
        "version" => {
            host.show_status(&format!("Kimi Code v{}", host.dispatch_state().version));
            Ok(())
        }
        "new" => {
            host.create_new_session().await?;
            host.request_render();
            Ok(())
        }
        "sessions" => host.show_session_picker().await,
        "tasks" => host.show_tasks_browser().await,
        _ => {
            host.run_builtin_command(BuiltinCommandInvocation { name, args })
                .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Host {
        state: DispatchState,
        skills: IndexMap<String, String>,
        plugins: IndexMap<String, String>,
        telemetry: Vec<DispatchTelemetryEvent>,
        errors: Vec<String>,
        statuses: Vec<String>,
        normal: Vec<String>,
        skill_activations: Vec<(String, String)>,
        plugin_activations: Vec<(String, String, String)>,
        builtins: Vec<BuiltinCommandInvocation>,
        help: usize,
        new_sessions: usize,
        renders: usize,
        dance: bool,
    }

    impl Default for Host {
        fn default() -> Self {
            Self {
                state: DispatchState {
                    model: "model".to_owned(),
                    has_session: true,
                    is_streaming: false,
                    is_compacting: false,
                    version: "1.2.3".to_owned(),
                },
                skills: IndexMap::new(),
                plugins: IndexMap::new(),
                telemetry: Vec::new(),
                errors: Vec::new(),
                statuses: Vec::new(),
                normal: Vec::new(),
                skill_activations: Vec::new(),
                plugin_activations: Vec::new(),
                builtins: Vec::new(),
                help: 0,
                new_sessions: 0,
                renders: 0,
                dance: false,
            }
        }
    }

    #[async_trait(?Send)]
    impl SlashCommandDispatchHost for Host {
        fn dispatch_state(&self) -> DispatchState {
            self.state.clone()
        }
        fn skill_command_map(&self) -> &IndexMap<String, String> {
            &self.skills
        }
        fn plugin_command_map(&self) -> &IndexMap<String, String> {
            &self.plugins
        }
        fn track_dispatch(&mut self, event: DispatchTelemetryEvent) {
            self.telemetry.push(event);
        }
        fn show_error(&mut self, message: &str) {
            self.errors.push(message.to_owned());
        }
        fn show_status(&mut self, message: &str) {
            self.statuses.push(message.to_owned());
        }
        fn send_normal_user_input(&mut self, text: &str) {
            self.normal.push(text.to_owned());
        }
        fn send_skill_activation(&mut self, skill_name: &str, args: &str) {
            self.skill_activations
                .push((skill_name.to_owned(), args.to_owned()));
        }
        fn activate_plugin_command(&mut self, plugin_id: &str, command_name: &str, args: &str) {
            self.plugin_activations.push((
                plugin_id.to_owned(),
                command_name.to_owned(),
                args.to_owned(),
            ));
        }
        fn try_handle_dance_command(&mut self, _: &ParsedSlashInput) -> bool {
            self.dance
        }
        async fn stop(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn show_help_panel(&mut self) {
            self.help += 1;
        }
        async fn create_new_session(&mut self) -> Result<(), String> {
            self.new_sessions += 1;
            Ok(())
        }
        fn request_render(&mut self) {
            self.renders += 1;
        }
        async fn show_session_picker(&mut self) -> Result<(), String> {
            Ok(())
        }
        async fn show_tasks_browser(&mut self) -> Result<(), String> {
            Ok(())
        }
        async fn run_builtin_command(
            &mut self,
            invocation: BuiltinCommandInvocation,
        ) -> Result<(), String> {
            self.builtins.push(invocation);
            Ok(())
        }
    }

    #[tokio::test]
    async fn plain_and_unknown_slash_inputs_fall_through_to_messages() {
        let mut host = Host::default();
        dispatch_input(&mut host, "hello").await;
        dispatch_input(&mut host, "/unknown arg").await;
        assert_eq!(host.normal, ["hello", "/unknown arg"]);
    }

    #[tokio::test]
    async fn busy_command_is_blocked_and_tracked() {
        let mut host = Host::default();
        host.state.is_streaming = true;
        dispatch_input(&mut host, "/new").await;
        assert!(host.errors[0].contains("Cannot /new while streaming"));
        assert_eq!(host.telemetry[0].name, "input_command_invalid");
        assert_eq!(host.telemetry[0].properties["reason"], "blocked");
        assert_eq!(host.new_sessions, 0);
    }

    #[tokio::test]
    async fn skills_and_plugin_commands_require_model_and_session() {
        let mut host = Host::default();
        host.skills
            .insert("review".to_owned(), "review-skill".to_owned());
        host.plugins
            .insert("demo:run".to_owned(), "body".to_owned());
        dispatch_input(&mut host, "/review src").await;
        dispatch_input(&mut host, "/demo:run now").await;
        assert_eq!(
            host.skill_activations,
            [("review-skill".to_owned(), "src".to_owned())]
        );
        assert_eq!(
            host.plugin_activations,
            [("demo".to_owned(), "run".to_owned(), "now".to_owned())]
        );

        host.state.model.clear();
        dispatch_input(&mut host, "/review").await;
        assert_eq!(host.errors, [LLM_NOT_SET_MESSAGE]);
    }

    #[tokio::test]
    async fn clear_alias_tracks_clear_and_runs_new_session() {
        let mut host = Host::default();
        dispatch_input(&mut host, "/clear").await;
        assert_eq!(
            host.telemetry
                .iter()
                .map(|event| event.name.as_str())
                .collect::<Vec<_>>(),
            ["input_command", "clear"]
        );
        assert_eq!((host.new_sessions, host.renders), (1, 1));
    }

    #[tokio::test]
    async fn core_builtins_are_local_and_other_builtins_are_typed_invocations() {
        let mut host = Host::default();
        dispatch_input(&mut host, "/help").await;
        dispatch_input(&mut host, "/version").await;
        dispatch_input(&mut host, "/model next").await;
        assert_eq!(host.help, 1);
        assert_eq!(host.statuses, ["Kimi Code v1.2.3"]);
        assert_eq!(
            host.builtins,
            [BuiltinCommandInvocation {
                name: "model",
                args: "next".to_owned()
            }]
        );
    }

    #[tokio::test]
    async fn dance_claims_unknown_command_before_message_fallback() {
        let mut host = Host {
            dance: true,
            ..Host::default()
        };
        dispatch_input(&mut host, "/dance").await;
        assert!(host.normal.is_empty());
    }
}
