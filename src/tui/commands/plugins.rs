use std::path::{Component as PathComponent, Path, PathBuf};

use async_trait::async_trait;

use crate::{
    sdk::types::{PluginInfo, PluginSource, PluginSummary},
    tui::{
        components::{
            dialogs::{PluginMcpSelection, PluginsPanelSelection, PluginsPanelTabId},
            messages::plugins_status_panel::{build_plugins_info_lines, build_plugins_list_lines},
        },
        utils::plugin_source_label::{format_plugin_source_label, is_official_plugin_source},
    },
};

const PLUGIN_RELOAD_HINT: &str = "Run /new or /reload to apply plugin changes.";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PluginReloadSummary {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PluginPickerRequest {
    pub selected_id: Option<String>,
    pub plugin_hint: Option<(String, String)>,
    pub initial_tab: Option<PluginsPanelTabId>,
    pub marketplace_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PluginMcpPickerRequest {
    pub plugin_id: String,
    pub selected_server: Option<String>,
    pub server_hint: Option<(String, String)>,
}

#[async_trait(?Send)]
pub trait PluginsCommandHost {
    fn work_dir(&self) -> &Path;
    async fn list_plugins(&self) -> Result<Vec<PluginSummary>, String>;
    async fn get_plugin_info(&self, id: &str) -> Result<PluginInfo, String>;
    async fn set_plugin_enabled(&mut self, id: &str, enabled: bool) -> Result<(), String>;
    async fn set_plugin_mcp_server_enabled(
        &mut self,
        id: &str,
        server: &str,
        enabled: bool,
    ) -> Result<(), String>;
    async fn install_plugin(&mut self, source: &str) -> Result<PluginSummary, String>;
    async fn remove_plugin(&mut self, id: &str) -> Result<(), String>;
    async fn reload_plugins(&mut self) -> Result<PluginReloadSummary, String>;

    /// The runtime binds this request to `PluginsPanelComponent`, including
    /// marketplace loading and in-place rendering while it awaits a selection.
    async fn pick_plugin(
        &mut self,
        installed: Vec<PluginSummary>,
        request: PluginPickerRequest,
    ) -> Option<PluginsPanelSelection>;
    async fn pick_plugin_mcp(
        &mut self,
        info: PluginInfo,
        request: PluginMcpPickerRequest,
    ) -> Option<PluginMcpSelection>;
    async fn confirm_plugin_remove(&mut self, id: &str, display_name: &str) -> bool;
    async fn confirm_plugin_trust(&mut self, label: &str) -> bool;

    fn begin_progress(&mut self, label: &str) -> u64;
    fn finish_progress(&mut self, id: u64, ok: bool, label: &str);
    fn add_report_panel(&mut self, title: &str, lines: Vec<String>);
    fn request_render(&mut self);
    fn open_url(&mut self, url: &str);
    fn show_status(&mut self, message: &str);
    fn show_warning(&mut self, message: &str);
    fn show_error(&mut self, message: &str);
}

// Original: `src/tui/commands/plugins.ts`, `handlePluginsCommand()`.
pub async fn handle_plugins_command(host: &mut impl PluginsCommandHost, raw_args: &str) {
    let args = raw_args.split_whitespace().collect::<Vec<_>>();
    let sub = args.first().copied();
    let rest = args.get(1..).unwrap_or_default();
    let result = match sub {
        None => show_plugins_picker(host, PluginPickerRequest::default()).await,
        Some("list") => render_plugins_list(host).await,
        Some("install") => {
            let source = rest.join(" ").trim().to_owned();
            if source.is_empty() {
                host.show_error("Usage: /plugins install <local-path-or-zip-url>");
                return;
            }
            install_from_command(host, &source).await
        }
        Some("marketplace") => {
            let source = rest.join(" ").trim().to_owned();
            show_plugins_picker(
                host,
                PluginPickerRequest {
                    initial_tab: Some(if source.is_empty() {
                        PluginsPanelTabId::Official
                    } else {
                        PluginsPanelTabId::ThirdParty
                    }),
                    marketplace_source: (!source.is_empty()).then_some(source),
                    ..PluginPickerRequest::default()
                },
            )
            .await
        }
        Some("info") => match rest.first() {
            Some(id) => render_plugin_info(host, id).await,
            None => show_plugins_picker(host, PluginPickerRequest::default()).await,
        },
        Some("mcp") => handle_mcp_subcommand(host, rest).await,
        Some(action @ ("enable" | "disable")) => match rest.first() {
            Some(id) => apply_plugin_enabled(host, id, action == "enable", true)
                .await
                .map(|_| ()),
            None => show_plugins_picker(host, PluginPickerRequest::default()).await,
        },
        Some("remove") => match rest.first() {
            Some(id) => remove_with_confirmation(host, id).await,
            None => {
                host.show_error("Usage: /plugins remove <id>");
                return;
            }
        },
        Some("reload") => reload_plugins(host).await,
        Some(id) => match host.list_plugins().await {
            Ok(plugins) if plugins.iter().any(|plugin| plugin.id == id) => {
                render_plugin_info(host, id).await
            }
            Ok(_) => {
                host.show_error(&format!(
                    "Unknown /plugins action: {id}. Run /plugins to choose interactively."
                ));
                return;
            }
            Err(error) => Err(error),
        },
    };
    if let Err(error) = result {
        host.show_error(&format!(
            "/plugins {} failed: {error}",
            sub.unwrap_or_default()
        ));
    }
}

async fn handle_mcp_subcommand(
    host: &mut impl PluginsCommandHost,
    args: &[&str],
) -> Result<(), String> {
    let [action @ ("enable" | "disable"), id, server, ..] = args else {
        host.show_error("Usage: /plugins mcp enable|disable <id> <server>");
        return Ok(());
    };
    host.set_plugin_mcp_server_enabled(id, server, *action == "enable")
        .await?;
    host.show_status(&format!(
        "{} MCP server {server} for {id}. Run /reload or /new to apply.",
        if *action == "enable" {
            "Enabled"
        } else {
            "Disabled"
        }
    ));
    Ok(())
}

async fn install_from_command(
    host: &mut impl PluginsCommandHost,
    source: &str,
) -> Result<(), String> {
    if !confirm_install_trust(host, source, is_official_plugin_source(source)).await {
        host.show_status("Install cancelled.");
        return Ok(());
    }
    let progress = host.begin_progress(&format!(
        "Installing plugin from {}…",
        truncate_for_status(source)
    ));
    match install_plugin_from_source(host, source).await {
        Ok(()) => {
            host.finish_progress(progress, true, "Install finished — see details below.");
            Ok(())
        }
        Err(error) => {
            host.finish_progress(progress, false, &format!("Install failed: {error}"));
            Err(error)
        }
    }
}

async fn show_plugins_picker(
    host: &mut impl PluginsCommandHost,
    mut request: PluginPickerRequest,
) -> Result<(), String> {
    loop {
        let installed = host.list_plugins().await?;
        let Some(selection) = host.pick_plugin(installed, request.clone()).await else {
            return Ok(());
        };
        request = match selection {
            PluginsPanelSelection::Toggle { id, enabled } => {
                let hint = apply_plugin_enabled(host, &id, enabled, false).await?;
                PluginPickerRequest {
                    initial_tab: Some(PluginsPanelTabId::Installed),
                    selected_id: Some(id.clone()),
                    plugin_hint: Some((id, hint)),
                    ..PluginPickerRequest::default()
                }
            }
            PluginsPanelSelection::Remove { id } => {
                let display_name = host
                    .get_plugin_info(&id)
                    .await
                    .map(|info| info.display_name)
                    .unwrap_or_else(|_| id.clone());
                if host.confirm_plugin_remove(&id, &display_name).await {
                    remove_plugin(host, &id).await?;
                    PluginPickerRequest {
                        initial_tab: Some(PluginsPanelTabId::Installed),
                        ..PluginPickerRequest::default()
                    }
                } else {
                    host.show_status(&format!("Remove cancelled: {id}."));
                    PluginPickerRequest {
                        initial_tab: Some(PluginsPanelTabId::Installed),
                        selected_id: Some(id),
                        ..PluginPickerRequest::default()
                    }
                }
            }
            PluginsPanelSelection::Mcp { id } => {
                show_plugin_mcp_picker(host, &id).await?;
                PluginPickerRequest {
                    selected_id: Some(id),
                    ..PluginPickerRequest::default()
                }
            }
            PluginsPanelSelection::Details { id } => {
                return render_plugin_info(host, &id).await;
            }
            PluginsPanelSelection::Reload => {
                reload_plugins(host).await?;
                PluginPickerRequest {
                    initial_tab: Some(PluginsPanelTabId::Installed),
                    ..PluginPickerRequest::default()
                }
            }
            PluginsPanelSelection::Install { entry } => {
                install_from_panel(
                    host,
                    &entry.source,
                    &entry.display_name,
                    is_official_plugin_source(&entry.source),
                )
                .await?;
                return Ok(());
            }
            PluginsPanelSelection::InstallSource { source } => {
                let official = is_official_plugin_source(&source);
                install_from_panel(host, &source, &source, official).await?;
                return Ok(());
            }
            PluginsPanelSelection::OpenUrl { url, label } => {
                host.open_url(&url);
                host.show_status(&format!("Opening the {label} page in your browser…"));
                host.show_status(&format!("If it did not open, visit {url}"));
                return Ok(());
            }
        };
    }
}

async fn show_plugin_mcp_picker(
    host: &mut impl PluginsCommandHost,
    plugin_id: &str,
) -> Result<(), String> {
    let mut request = PluginMcpPickerRequest {
        plugin_id: plugin_id.to_owned(),
        ..PluginMcpPickerRequest::default()
    };
    loop {
        let info = host.get_plugin_info(plugin_id).await?;
        let Some(selection) = host.pick_plugin_mcp(info, request.clone()).await else {
            return Ok(());
        };
        match selection {
            PluginMcpSelection::Toggle {
                plugin_id,
                server,
                enabled,
            } => {
                host.set_plugin_mcp_server_enabled(&plugin_id, &server, enabled)
                    .await?;
                request = PluginMcpPickerRequest {
                    plugin_id,
                    selected_server: Some(server.clone()),
                    server_hint: Some((server, plugin_inline_change_hint().to_owned())),
                };
            }
            PluginMcpSelection::Back { .. } => return Ok(()),
        }
    }
}

async fn confirm_install_trust(
    host: &mut impl PluginsCommandHost,
    label: &str,
    official: bool,
) -> bool {
    official || host.confirm_plugin_trust(label).await
}

async fn install_from_panel(
    host: &mut impl PluginsCommandHost,
    source: &str,
    label: &str,
    official: bool,
) -> Result<(), String> {
    if !confirm_install_trust(host, label, official).await {
        host.show_status(&format!("Install cancelled: {label}."));
        return Ok(());
    }
    host.show_status(&format!(
        "Installing or updating {label} from marketplace..."
    ));
    install_plugin_from_source(host, source).await
}

async fn apply_plugin_enabled(
    host: &mut impl PluginsCommandHost,
    id: &str,
    enabled: bool,
    show_status: bool,
) -> Result<String, String> {
    host.set_plugin_enabled(id, enabled).await?;
    let info = host.get_plugin_info(id).await.ok();
    let mcp_hint = if enabled
        && info
            .as_ref()
            .is_some_and(|info| info.mcp_server_count > info.enabled_mcp_server_count)
    {
        format!(" Some MCP servers are disabled; re-enable with /plugins mcp enable {id} <server>.")
    } else {
        String::new()
    };
    if show_status {
        host.show_status(&format!(
            "{} {id}. Run /reload or /new to apply.{mcp_hint}",
            if enabled { "Enabled" } else { "Disabled" }
        ));
    }
    Ok(format!(
        "{}{}",
        plugin_inline_change_hint(),
        if mcp_hint.is_empty() {
            ""
        } else {
            " · MCP servers disabled"
        }
    ))
}

async fn remove_with_confirmation(
    host: &mut impl PluginsCommandHost,
    id: &str,
) -> Result<(), String> {
    let display_name = host
        .get_plugin_info(id)
        .await
        .map(|info| info.display_name)
        .unwrap_or_else(|_| id.to_owned());
    if !host.confirm_plugin_remove(id, &display_name).await {
        host.show_status(&format!("Remove cancelled: {id}."));
        return Ok(());
    }
    remove_plugin(host, id).await
}

async fn remove_plugin(host: &mut impl PluginsCommandHost, id: &str) -> Result<(), String> {
    host.remove_plugin(id).await?;
    host.show_status(&format!("Removed {id}."));
    host.show_warning(PLUGIN_RELOAD_HINT);
    Ok(())
}

async fn render_plugins_list(host: &mut impl PluginsCommandHost) -> Result<(), String> {
    let plugins = host.list_plugins().await?;
    let title = format!(" Plugins ({}) ", plugins.len());
    host.add_report_panel(&title, build_plugins_list_lines(&plugins));
    host.request_render();
    Ok(())
}

async fn render_plugin_info(host: &mut impl PluginsCommandHost, id: &str) -> Result<(), String> {
    let info = host.get_plugin_info(id).await?;
    let title = format!(" {} ", info.id);
    host.add_report_panel(&title, build_plugins_info_lines(&info));
    host.request_render();
    Ok(())
}

async fn install_plugin_from_source(
    host: &mut impl PluginsCommandHost,
    source: &str,
) -> Result<(), String> {
    let before = host.list_plugins().await?;
    let resolved = resolve_plugin_install_source(source, host.work_dir());
    let summary = host.install_plugin(&resolved).await?;
    show_plugin_install_result(host, &before, &summary);
    Ok(())
}

fn show_plugin_install_result(
    host: &mut impl PluginsCommandHost,
    before: &[PluginSummary],
    summary: &PluginSummary,
) {
    let previous = before.iter().find(|plugin| plugin.id == summary.id);
    let server_word = if summary.mcp_server_count == 1 {
        "server"
    } else {
        "servers"
    };
    let mcp_hint = if summary.mcp_server_count > 0 {
        format!(
            " Declares {} MCP {server_word}; enabled by default and configurable from /plugins.",
            summary.mcp_server_count
        )
    } else {
        String::new()
    };
    host.show_status(&format!(
        "{} ({}).{mcp_hint}",
        describe_install_action(previous, summary),
        summary.id
    ));
    host.show_warning(PLUGIN_RELOAD_HINT);
}

pub fn describe_install_action(previous: Option<&PluginSummary>, next: &PluginSummary) -> String {
    let source_label = format_plugin_source_label(next);
    let version = |previous: Option<&str>, current: Option<&str>| match (previous, current) {
        (Some(previous), Some(current)) if previous != current => {
            format!(" {previous} → {current}")
        }
        (Some(previous), None) => format!(" {previous} → -"),
        (_, Some(current)) => format!(" {current}"),
        _ => String::new(),
    };
    let Some(previous) = previous else {
        return format!(
            "Installed {}{} {}",
            next.display_name,
            version(None, next.version.as_deref()),
            source_phrase(&source_label)
        );
    };
    if source_identity(previous) != source_identity(next) {
        return format!(
            "Migrated {}: {} → {}{}",
            next.display_name,
            format_plugin_source_label(previous),
            source_label,
            version(previous.version.as_deref(), next.version.as_deref())
        );
    }
    format!(
        "Updated {}{} {}",
        next.display_name,
        version(previous.version.as_deref(), next.version.as_deref()),
        source_phrase(&source_label)
    )
}

fn source_phrase(source_label: &str) -> String {
    if source_label.starts_with("via ") {
        source_label.to_owned()
    } else {
        format!("from {source_label}")
    }
}

fn source_identity(plugin: &PluginSummary) -> String {
    if plugin.source == PluginSource::Github
        && let Some(github) = &plugin.github
    {
        return format!("github:{}/{}", github.owner, github.repo);
    }
    plugin.source.as_str().to_owned()
}

pub fn truncate_for_status(input: &str) -> String {
    const MAX_UTF16: usize = 80;
    if input.encode_utf16().count() <= MAX_UTF16 {
        return input.to_owned();
    }
    let mut result = String::new();
    for character in input.chars() {
        if result.encode_utf16().count() + character.len_utf16() > MAX_UTF16 - 1 {
            break;
        }
        result.push(character);
    }
    result.push('…');
    result
}

async fn reload_plugins(host: &mut impl PluginsCommandHost) -> Result<(), String> {
    let summary = host.reload_plugins().await?;
    let errors = if summary.errors.is_empty() {
        String::new()
    } else {
        format!(" ({} errors)", summary.errors.len())
    };
    host.show_status(&format!(
        "Reload: +{} -{}{errors}",
        summary.added.len(),
        summary.removed.len()
    ));
    Ok(())
}

pub fn resolve_plugin_install_source(source: &str, work_dir: &Path) -> String {
    let source = source.trim();
    if source.starts_with("http://") || source.starts_with("https://") {
        return source.to_owned();
    }
    let path = if source == "~" {
        dirs::home_dir().unwrap_or_else(|| work_dir.to_owned())
    } else if let Some(remainder) = source
        .strip_prefix("~/")
        .or_else(|| source.strip_prefix("~\\"))
    {
        dirs::home_dir()
            .unwrap_or_else(|| work_dir.to_owned())
            .join(remainder)
    } else {
        let path = PathBuf::from(source);
        if path.is_absolute() {
            path
        } else {
            work_dir.join(path)
        }
    };
    normalize_path(&path).to_string_lossy().into_owned()
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            PathComponent::CurDir => {}
            PathComponent::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

const fn plugin_inline_change_hint() -> &'static str {
    "run /reload or /new to apply"
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use crate::sdk::types::{
        PluginGithubMetadata, PluginGithubRef, PluginGithubRefKind, PluginState,
    };

    use super::*;

    fn plugin(id: &str, version: Option<&str>, source: PluginSource) -> PluginSummary {
        PluginSummary {
            id: id.to_owned(),
            display_name: "Example".to_owned(),
            version: version.map(str::to_owned),
            enabled: true,
            state: PluginState::Ok,
            skill_count: 1,
            mcp_server_count: 0,
            enabled_mcp_server_count: 0,
            hook_count: 0,
            command_count: 0,
            has_errors: false,
            source,
            original_source: None,
            github: None,
        }
    }

    #[derive(Default)]
    struct Host {
        plugins: Vec<PluginSummary>,
        picker: VecDeque<Option<PluginsPanelSelection>>,
        enabled: Vec<(String, bool)>,
        mcp_enabled: Vec<(String, String, bool)>,
        removed: Vec<String>,
        statuses: Vec<String>,
        warnings: Vec<String>,
        errors: Vec<String>,
        panels: Vec<(String, Vec<String>)>,
        renders: usize,
        confirms: bool,
    }

    #[async_trait(?Send)]
    impl PluginsCommandHost for Host {
        fn work_dir(&self) -> &Path {
            Path::new("C:/work")
        }
        async fn list_plugins(&self) -> Result<Vec<PluginSummary>, String> {
            Ok(self.plugins.clone())
        }
        async fn get_plugin_info(&self, id: &str) -> Result<PluginInfo, String> {
            Err(format!("missing {id}"))
        }
        async fn set_plugin_enabled(&mut self, id: &str, enabled: bool) -> Result<(), String> {
            self.enabled.push((id.to_owned(), enabled));
            Ok(())
        }
        async fn set_plugin_mcp_server_enabled(
            &mut self,
            id: &str,
            server: &str,
            enabled: bool,
        ) -> Result<(), String> {
            self.mcp_enabled
                .push((id.to_owned(), server.to_owned(), enabled));
            Ok(())
        }
        async fn install_plugin(&mut self, _: &str) -> Result<PluginSummary, String> {
            Err("unused".to_owned())
        }
        async fn remove_plugin(&mut self, id: &str) -> Result<(), String> {
            self.removed.push(id.to_owned());
            Ok(())
        }
        async fn reload_plugins(&mut self) -> Result<PluginReloadSummary, String> {
            Ok(PluginReloadSummary::default())
        }
        async fn pick_plugin(
            &mut self,
            _: Vec<PluginSummary>,
            _: PluginPickerRequest,
        ) -> Option<PluginsPanelSelection> {
            self.picker.pop_front().flatten()
        }
        async fn pick_plugin_mcp(
            &mut self,
            _: PluginInfo,
            _: PluginMcpPickerRequest,
        ) -> Option<PluginMcpSelection> {
            None
        }
        async fn confirm_plugin_remove(&mut self, _: &str, _: &str) -> bool {
            self.confirms
        }
        async fn confirm_plugin_trust(&mut self, _: &str) -> bool {
            self.confirms
        }
        fn begin_progress(&mut self, _: &str) -> u64 {
            1
        }
        fn finish_progress(&mut self, _: u64, _: bool, _: &str) {}
        fn add_report_panel(&mut self, title: &str, lines: Vec<String>) {
            self.panels.push((title.to_owned(), lines));
        }
        fn request_render(&mut self) {
            self.renders += 1;
        }
        fn open_url(&mut self, _: &str) {}
        fn show_status(&mut self, message: &str) {
            self.statuses.push(message.to_owned());
        }
        fn show_warning(&mut self, message: &str) {
            self.warnings.push(message.to_owned());
        }
        fn show_error(&mut self, message: &str) {
            self.errors.push(message.to_owned());
        }
    }

    #[tokio::test]
    async fn command_validates_mcp_arguments_and_applies_valid_toggle() {
        let mut host = Host::default();
        handle_plugins_command(&mut host, "mcp enable plugin server").await;
        assert_eq!(
            host.mcp_enabled,
            [("plugin".to_owned(), "server".to_owned(), true)]
        );
        assert!(host.statuses[0].contains("Enabled MCP server server"));

        handle_plugins_command(&mut host, "mcp maybe plugin server").await;
        assert_eq!(
            host.errors.last().map(String::as_str),
            Some("Usage: /plugins mcp enable|disable <id> <server>")
        );
    }

    #[tokio::test]
    async fn picker_toggle_reopens_then_cancel_returns() {
        let mut host = Host::default();
        host.plugins
            .push(plugin("demo", None, PluginSource::LocalPath));
        host.picker.push_back(Some(PluginsPanelSelection::Toggle {
            id: "demo".to_owned(),
            enabled: false,
        }));
        host.picker.push_back(None);
        handle_plugins_command(&mut host, "").await;
        assert_eq!(host.enabled, [("demo".to_owned(), false)]);
        assert!(host.errors.is_empty());
    }

    #[tokio::test]
    async fn remove_cancellation_preserves_plugin() {
        let mut host = Host::default();
        handle_plugins_command(&mut host, "remove demo").await;
        assert!(host.removed.is_empty());
        assert_eq!(host.statuses, ["Remove cancelled: demo."]);
    }

    #[test]
    fn installation_description_distinguishes_install_update_and_migration() {
        let installed = plugin("demo", Some("1.0.0"), PluginSource::LocalPath);
        assert!(describe_install_action(None, &installed).starts_with("Installed Example 1.0.0"));
        let mut updated = installed.clone();
        updated.version = Some("2.0.0".to_owned());
        assert!(describe_install_action(Some(&installed), &updated).contains("1.0.0 → 2.0.0"));
        updated.source = PluginSource::Github;
        updated.github = Some(PluginGithubMetadata {
            owner: "owner".to_owned(),
            repo: "repo".to_owned(),
            reference: PluginGithubRef {
                kind: PluginGithubRefKind::Tag,
                value: "v2".to_owned(),
            },
            installed_sha: None,
        });
        assert!(
            describe_install_action(Some(&installed), &updated).starts_with("Migrated Example")
        );
    }

    #[test]
    fn source_resolution_preserves_urls_and_resolves_relative_paths() {
        assert_eq!(
            resolve_plugin_install_source("https://example/a.zip", Path::new("C:/work")),
            "https://example/a.zip"
        );
        assert_eq!(
            resolve_plugin_install_source("plugins/../demo", Path::new("C:/work")),
            "C:\\work\\demo"
        );
        assert_eq!(
            truncate_for_status(&"界".repeat(81)).encode_utf16().count(),
            80
        );
    }
}
