use crate::sdk::types::{McpServerStatus, McpServerStatusSnapshot};

pub const MCP_STARTUP_STATUS_ROW_LIMIT: usize = 4;

fn startup_status_priority(status: McpServerStatus) -> u8 {
    match status {
        McpServerStatus::Failed => 0,
        McpServerStatus::NeedsAuth => 1,
        McpServerStatus::Pending => 2,
        McpServerStatus::Connected => 3,
        McpServerStatus::Disabled => 4,
    }
}

/// Original:
///   apps/kimi-code/src/tui/utils/mcp-server-status.ts
///   selectMcpStartupStatusRows()
pub fn select_mcp_startup_status_rows(
    servers: &[McpServerStatusSnapshot],
) -> Vec<McpServerStatusSnapshot> {
    let mut selected = servers
        .iter()
        .filter(|server| server.status != McpServerStatus::Disabled)
        .cloned()
        .collect::<Vec<_>>();
    selected.sort_by_key(|server| startup_status_priority(server.status));
    selected.truncate(MCP_STARTUP_STATUS_ROW_LIMIT);
    selected
}

pub fn format_mcp_startup_status_summary(servers: &[McpServerStatusSnapshot]) -> String {
    let mut failed = 0;
    let mut needs_auth = 0;
    let mut connecting = 0;
    let mut connected = 0;
    let mut disabled = 0;
    for server in servers {
        match server.status {
            McpServerStatus::Failed => failed += 1,
            McpServerStatus::NeedsAuth => needs_auth += 1,
            McpServerStatus::Pending => connecting += 1,
            McpServerStatus::Connected => connected += 1,
            McpServerStatus::Disabled => disabled += 1,
        }
    }
    [
        (failed, "failed"),
        (needs_auth, "need auth"),
        (connecting, "connecting"),
        (connected, "connected"),
        (disabled, "disabled"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, label)| format!("{count} {label}"))
    .collect::<Vec<_>>()
    .join(", ")
}

pub fn mcp_server_status_key(
    server: &McpServerStatusSnapshot,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&(
        server.status,
        server.transport,
        server.tool_count,
        &server.error,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::types::McpServerTransport;

    fn server(name: &str, status: McpServerStatus) -> McpServerStatusSnapshot {
        McpServerStatusSnapshot {
            name: name.to_owned(),
            transport: McpServerTransport::Http,
            status,
            tool_count: 2,
            error: None,
        }
    }

    #[test]
    fn selects_four_non_disabled_rows_by_stable_priority() {
        let servers = [
            server("connected", McpServerStatus::Connected),
            server("failed-one", McpServerStatus::Failed),
            server("disabled", McpServerStatus::Disabled),
            server("auth", McpServerStatus::NeedsAuth),
            server("pending", McpServerStatus::Pending),
            server("failed-two", McpServerStatus::Failed),
        ];
        let selected = select_mcp_startup_status_rows(&servers);

        assert_eq!(
            selected
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
            ["failed-one", "failed-two", "auth", "pending"]
        );
    }

    #[test]
    fn summarizes_each_nonzero_status_in_display_order() {
        let servers = [
            server("failed", McpServerStatus::Failed),
            server("auth", McpServerStatus::NeedsAuth),
            server("pending", McpServerStatus::Pending),
            server("connected-one", McpServerStatus::Connected),
            server("connected-two", McpServerStatus::Connected),
            server("disabled", McpServerStatus::Disabled),
        ];
        assert_eq!(
            format_mcp_startup_status_summary(&servers),
            "1 failed, 1 need auth, 1 connecting, 2 connected, 1 disabled"
        );
        assert_eq!(format_mcp_startup_status_summary(&[]), "");
    }

    #[test]
    fn status_key_tracks_render_relevant_fields_but_not_name() {
        let mut first = server("one", McpServerStatus::Failed);
        first.error = Some("boom".to_owned());
        let mut second = first.clone();
        second.name = "two".to_owned();
        let first_key = mcp_server_status_key(&first).ok();
        let second_key = mcp_server_status_key(&second).ok();
        assert_eq!(first_key, second_key);

        second.tool_count = 3;
        assert_ne!(first_key, mcp_server_status_key(&second).ok());
    }
}
