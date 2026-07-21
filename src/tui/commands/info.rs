use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::{
    oauth::managed_usage::{ParsedManagedUsage, is_managed_kimi_code},
    sdk::{
        model_alias::ModelAlias,
        types::{
            McpServerStatusSnapshot, PermissionMode, SessionStatus, SessionUsage, ThinkingEffort,
        },
    },
    tui::components::{
        Component,
        messages::{
            mcp_status_panel::build_mcp_status_report_lines,
            status_panel::{StatusReportOptions, build_status_report_lines},
            usage_panel::{UsagePanelComponent, UsageReportOptions, build_usage_report_lines},
        },
    },
};

#[derive(Debug, Clone, PartialEq)]
pub struct InfoAppState {
    pub version: String,
    pub model: String,
    pub work_dir: String,
    pub session_id: String,
    pub session_title: Option<String>,
    pub thinking_effort: ThinkingEffort,
    pub permission_mode: PermissionMode,
    pub plan_mode: bool,
    pub context_usage: f64,
    pub context_tokens: u64,
    pub max_context_tokens: u64,
    pub available_models: BTreeMap<String, ModelAlias>,
}

#[async_trait(?Send)]
pub trait InfoCommandHost {
    fn info_app_state(&self) -> InfoAppState;
    async fn get_session_usage(&self) -> Result<SessionUsage, String>;
    async fn get_runtime_status(&self) -> Result<SessionStatus, String>;
    async fn get_managed_usage(&self, provider_key: &str) -> Result<ParsedManagedUsage, String>;
    async fn list_mcp_servers(&self) -> Result<Vec<McpServerStatusSnapshot>, String>;
    fn add_transcript_component(&mut self, component: Box<dyn Component>);
    fn request_render(&mut self);
    fn show_error(&mut self, message: &str);
}

#[derive(Debug)]
struct ReportResult<T> {
    value: Option<T>,
    error: Option<String>,
}

impl<T> ReportResult<T> {
    fn from_result(result: Result<T, String>) -> Self {
        match result {
            Ok(value) => Self {
                value: Some(value),
                error: None,
            },
            Err(error) => Self {
                value: None,
                error: Some(error),
            },
        }
    }
}

fn provider_key(state: &InfoAppState) -> Option<&str> {
    state
        .available_models
        .get(&state.model)
        .map(|model| model.provider.as_str())
}

async fn load_managed_usage_report(
    host: &impl InfoCommandHost,
    state: &InfoAppState,
) -> Option<ReportResult<ParsedManagedUsage>> {
    let provider_key = provider_key(state).filter(|key| is_managed_kimi_code(Some(key)))?;
    Some(ReportResult::from_result(
        host.get_managed_usage(provider_key).await,
    ))
}

// Original: `src/tui/commands/info.ts`, `showUsage()`.
pub async fn show_usage(host: &mut impl InfoCommandHost) {
    let state = host.info_app_state();
    let session_usage = ReportResult::from_result(host.get_session_usage().await);
    let managed_usage = load_managed_usage_report(host, &state).await;
    let lines = build_usage_report_lines(UsageReportOptions {
        session_usage: session_usage.value.as_ref(),
        session_usage_error: session_usage.error.as_deref(),
        context_usage: state.context_usage,
        context_tokens: state.context_tokens,
        max_context_tokens: state.max_context_tokens,
        managed_usage: managed_usage
            .as_ref()
            .and_then(|result| result.value.as_ref()),
        managed_usage_error: managed_usage
            .as_ref()
            .and_then(|result| result.error.as_deref()),
    });
    host.add_transcript_component(Box::new(UsagePanelComponent::usage(move || lines.clone())));
    host.request_render();
}

// Original: `showStatusReport()`.
pub async fn show_status_report(host: &mut impl InfoCommandHost) {
    let state = host.info_app_state();
    let (runtime_status, managed_usage) = tokio::join!(
        async { ReportResult::from_result(host.get_runtime_status().await) },
        load_managed_usage_report(host, &state),
    );
    let lines = build_status_report_lines(StatusReportOptions {
        version: &state.version,
        model: &state.model,
        work_dir: &state.work_dir,
        session_id: &state.session_id,
        session_title: state.session_title.as_deref(),
        thinking_effort: &state.thinking_effort,
        permission_mode: state.permission_mode,
        plan_mode: state.plan_mode,
        context_usage: state.context_usage,
        context_tokens: state.context_tokens,
        max_context_tokens: state.max_context_tokens,
        available_models: &state.available_models,
        status: runtime_status.value.as_ref(),
        status_error: runtime_status.error.as_deref(),
        managed_usage: managed_usage
            .as_ref()
            .and_then(|result| result.value.as_ref()),
        managed_usage_error: managed_usage
            .as_ref()
            .and_then(|result| result.error.as_deref()),
    });
    host.add_transcript_component(Box::new(UsagePanelComponent::new(
        move || lines.clone(),
        crate::tui::theme::ColorToken::Primary,
        " Status ",
    )));
    host.request_render();
}

// Original: `showMcpServers()`.
pub async fn show_mcp_servers(host: &mut impl InfoCommandHost) {
    let servers = match host.list_mcp_servers().await {
        Ok(servers) => servers,
        Err(error) => {
            host.show_error(&format!("Failed to load MCP servers: {error}"));
            return;
        }
    };
    let title = if servers.is_empty() {
        " MCP ".to_owned()
    } else {
        format!(" MCP ({}) ", servers.len())
    };
    let lines = build_mcp_status_report_lines(&servers);
    host.add_transcript_component(Box::new(UsagePanelComponent::new(
        move || lines.clone(),
        crate::tui::theme::ColorToken::Primary,
        title,
    )));
    host.request_render();
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crate::{
        oauth::managed_usage::UsageRow,
        sdk::{model_alias::ModelProtocol, types::McpServerTransport},
    };

    use super::*;

    struct Host {
        state: InfoAppState,
        usage: Result<SessionUsage, String>,
        status: Result<SessionStatus, String>,
        managed: Result<ParsedManagedUsage, String>,
        mcp: Result<Vec<McpServerStatusSnapshot>, String>,
        panels: Vec<Box<dyn Component>>,
        renders: usize,
        errors: Vec<String>,
        managed_calls: Mutex<Vec<String>>,
    }

    #[async_trait(?Send)]
    impl InfoCommandHost for Host {
        fn info_app_state(&self) -> InfoAppState {
            self.state.clone()
        }

        async fn get_session_usage(&self) -> Result<SessionUsage, String> {
            self.usage.clone()
        }

        async fn get_runtime_status(&self) -> Result<SessionStatus, String> {
            self.status.clone()
        }

        async fn get_managed_usage(
            &self,
            provider_key: &str,
        ) -> Result<ParsedManagedUsage, String> {
            self.managed_calls
                .lock()
                .expect("managed calls")
                .push(provider_key.to_owned());
            self.managed.clone()
        }

        async fn list_mcp_servers(&self) -> Result<Vec<McpServerStatusSnapshot>, String> {
            self.mcp.clone()
        }

        fn add_transcript_component(&mut self, component: Box<dyn Component>) {
            self.panels.push(component);
        }

        fn request_render(&mut self) {
            self.renders += 1;
        }

        fn show_error(&mut self, message: &str) {
            self.errors.push(message.to_owned());
        }
    }

    fn alias(provider: &str) -> ModelAlias {
        ModelAlias {
            provider: provider.to_owned(),
            model: "model".to_owned(),
            max_context_size: 128_000,
            max_output_size: None,
            capabilities: None,
            display_name: Some("Model".to_owned()),
            reasoning_key: None,
            protocol: Some(ModelProtocol::Anthropic),
            adaptive_thinking: None,
            support_efforts: None,
            default_effort: None,
            beta_api: None,
            overrides: None,
        }
    }

    fn host(provider: &str) -> Host {
        Host {
            state: InfoAppState {
                version: "1.2.3".to_owned(),
                model: "alias".to_owned(),
                work_dir: "C:/repo".to_owned(),
                session_id: "session-1".to_owned(),
                session_title: Some("Title".to_owned()),
                thinking_effort: ThinkingEffort::from("on"),
                permission_mode: PermissionMode::Manual,
                plan_mode: false,
                context_usage: 0.25,
                context_tokens: 32_000,
                max_context_tokens: 128_000,
                available_models: BTreeMap::from([("alias".to_owned(), alias(provider))]),
            },
            usage: Ok(SessionUsage::default()),
            status: Err("status unavailable".to_owned()),
            managed: Ok(ParsedManagedUsage {
                summary: Some(UsageRow {
                    label: "Weekly limit".to_owned(),
                    used: 3.0,
                    limit: 10.0,
                    reset_hint: None,
                }),
                limits: Vec::new(),
                extra_usage: None,
            }),
            mcp: Ok(Vec::new()),
            panels: Vec::new(),
            renders: 0,
            errors: Vec::new(),
            managed_calls: Mutex::new(Vec::new()),
        }
    }

    fn rendered_panel(host: &mut Host) -> String {
        host.panels[0].render(100).join("\n")
    }

    #[tokio::test]
    async fn usage_loads_managed_plan_for_managed_provider() {
        let mut host = host("managed:kimi-code");
        show_usage(&mut host).await;
        assert_eq!(
            *host.managed_calls.lock().expect("managed calls"),
            ["managed:kimi-code"]
        );
        assert_eq!((host.panels.len(), host.renders), (1, 1));
        let output = rendered_panel(&mut host);
        assert!(output.contains("Session usage"));
        assert!(output.contains("Weekly limit"));
    }

    #[tokio::test]
    async fn status_skips_managed_usage_for_other_provider_and_keeps_errors() {
        let mut host = host("openai");
        show_status_report(&mut host).await;
        assert!(host.managed_calls.lock().expect("managed calls").is_empty());
        let output = rendered_panel(&mut host);
        assert!(output.contains("status unavailable"));
        assert!(output.contains("v1.2.3"));
    }

    #[tokio::test]
    async fn mcp_error_is_reported_without_panel_or_render() {
        let mut host = host("openai");
        host.mcp = Err("session closed".to_owned());
        show_mcp_servers(&mut host).await;
        assert_eq!(host.errors, ["Failed to load MCP servers: session closed"]);
        assert!(host.panels.is_empty());
        assert_eq!(host.renders, 0);
    }

    #[tokio::test]
    async fn mcp_panel_title_includes_server_count() {
        let mut host = host("openai");
        host.mcp = Ok(vec![McpServerStatusSnapshot {
            name: "tools".to_owned(),
            transport: McpServerTransport::Stdio,
            status: crate::sdk::types::McpServerStatus::Connected,
            tool_count: 3,
            error: None,
        }]);
        show_mcp_servers(&mut host).await;
        let output = rendered_panel(&mut host);
        assert!(output.contains("MCP (1)"));
        assert!(output.contains("tools"));
    }
}
