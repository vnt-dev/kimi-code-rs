use crate::{
    sdk::types::{McpServerStatus, McpServerStatusSnapshot, McpServerTransport},
    tui::theme::{ColorToken, current_theme},
};

fn status_priority(status: McpServerStatus) -> u8 {
    match status {
        McpServerStatus::Failed => 0,
        McpServerStatus::NeedsAuth => 1,
        McpServerStatus::Pending => 2,
        McpServerStatus::Connected => 3,
        McpServerStatus::Disabled => 4,
    }
}

fn status_label(status: McpServerStatus) -> &'static str {
    match status {
        McpServerStatus::Connected => "connected",
        McpServerStatus::Pending => "pending",
        McpServerStatus::NeedsAuth => "needs auth",
        McpServerStatus::Failed => "failed",
        McpServerStatus::Disabled => "disabled",
    }
}

fn transport_label(transport: McpServerTransport) -> &'static str {
    match transport {
        McpServerTransport::Stdio => "stdio",
        McpServerTransport::Http => "http",
        McpServerTransport::Sse => "sse",
    }
}

fn status_token(status: McpServerStatus) -> ColorToken {
    match status {
        McpServerStatus::Connected => ColorToken::Success,
        McpServerStatus::Failed => ColorToken::Error,
        McpServerStatus::NeedsAuth | McpServerStatus::Pending => ColorToken::Warning,
        McpServerStatus::Disabled => ColorToken::TextDim,
    }
}

fn format_tool_count(server: &McpServerStatusSnapshot) -> String {
    if server.status == McpServerStatus::Disabled {
        "—".to_owned()
    } else {
        format!(
            "{} tool{}",
            server.tool_count,
            if server.tool_count == 1 { "" } else { "s" }
        )
    }
}

fn format_tools_available(count: usize) -> String {
    format!(
        "{count} tool{} available",
        if count == 1 { "" } else { "s" }
    )
}

fn format_error_line(error: &str) -> String {
    error.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn pad_end_utf16(text: &str, width: usize) -> String {
    let current = text.encode_utf16().count();
    format!("{text}{}", " ".repeat(width.saturating_sub(current)))
}

fn sorted_servers(servers: &[McpServerStatusSnapshot]) -> Vec<&McpServerStatusSnapshot> {
    let mut sorted = servers.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| {
        status_priority(left.status)
            .cmp(&status_priority(right.status))
            .then_with(|| left.name.cmp(&right.name))
    });
    sorted
}

fn build_summary(servers: &[&McpServerStatusSnapshot]) -> String {
    let mut counts = [0_usize; 5];
    let mut tools_available = 0;
    for server in servers {
        counts[usize::from(status_priority(server.status))] += 1;
        if server.status == McpServerStatus::Connected {
            tools_available += server.tool_count;
        }
    }
    let mut parts = Vec::new();
    for status in [
        McpServerStatus::Connected,
        McpServerStatus::Pending,
        McpServerStatus::NeedsAuth,
        McpServerStatus::Failed,
        McpServerStatus::Disabled,
    ] {
        let count = counts[usize::from(status_priority(status))];
        if count > 0 {
            parts.push(format!("{count} {}", status_label(status)));
        }
    }
    parts.push(format_tools_available(tools_available));
    parts.join(" · ")
}

/// Original:
///   apps/kimi-code/src/tui/components/messages/mcp-status-panel.ts
///   buildMcpStatusReportLines()
pub fn build_mcp_status_report_lines(servers: &[McpServerStatusSnapshot]) -> Vec<String> {
    let servers = sorted_servers(servers);
    let theme = current_theme();
    let mut lines = vec![theme.bold_fg(ColorToken::Primary, "Servers")];
    if servers.is_empty() {
        lines.push(theme.fg(
            ColorToken::TextDim,
            "  No MCP servers configured. Run /mcp-config to add one.",
        ));
        return lines;
    }

    let name_width = servers
        .iter()
        .map(|server| server.name.encode_utf16().count())
        .max()
        .unwrap_or(0)
        .max("Name".len());
    let status_width = servers
        .iter()
        .map(|server| status_label(server.status).len())
        .max()
        .unwrap_or(0)
        .max("Status".len());
    let transport_width = servers
        .iter()
        .map(|server| transport_label(server.transport).len())
        .max()
        .unwrap_or(0)
        .max("Transport".len());
    lines.push(format!(
        "  {}  {}  {}  {}",
        theme.fg(ColorToken::TextDim, &pad_end_utf16("Name", name_width)),
        theme.fg(ColorToken::TextDim, &format!("{:<status_width$}", "Status")),
        theme.fg(
            ColorToken::TextDim,
            &format!("{:<transport_width$}", "Transport")
        ),
        theme.fg(ColorToken::TextDim, "Tools")
    ));

    for server in &servers {
        lines.push(format!(
            "  {}  {}  {}  {}",
            theme.fg(ColorToken::Text, &pad_end_utf16(&server.name, name_width)),
            theme.fg(
                status_token(server.status),
                &format!("{:<status_width$}", status_label(server.status))
            ),
            theme.fg(
                ColorToken::TextDim,
                &format!("{:<transport_width$}", transport_label(server.transport))
            ),
            theme.fg(ColorToken::Text, &format_tool_count(server))
        ));
        if server.status == McpServerStatus::Failed
            && let Some(error) = server
                .error
                .as_deref()
                .filter(|error| !error.trim().is_empty())
        {
            lines.push(format!(
                "    {} {}",
                theme.fg(ColorToken::TextDim, "error:"),
                theme.fg(ColorToken::Error, &format_error_line(error))
            ));
        }
        if server.status == McpServerStatus::NeedsAuth {
            lines.push(format!(
                "    {} {}",
                theme.fg(ColorToken::TextDim, "action:"),
                theme.fg(
                    ColorToken::Text,
                    &format!("run /mcp-config login {}", server.name)
                )
            ));
        }
    }

    lines.push(String::new());
    lines.push(format!(
        "  {}",
        theme.fg(ColorToken::Text, &build_summary(&servers))
    ));
    lines.push(format!(
        "  {} {}",
        theme.fg(ColorToken::TextDim, "Configure with"),
        theme.fg(ColorToken::Text, "/mcp-config")
    ));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(
        name: &str,
        transport: McpServerTransport,
        status: McpServerStatus,
        tool_count: usize,
        error: Option<&str>,
    ) -> McpServerStatusSnapshot {
        McpServerStatusSnapshot {
            name: name.to_owned(),
            transport,
            status,
            tool_count,
            error: error.map(str::to_owned),
        }
    }

    fn strip_sgr(text: &str) -> String {
        let mut output = String::new();
        let mut escape = false;
        for character in text.chars() {
            if character == '\u{1b}' {
                escape = true;
            } else if escape && character == 'm' {
                escape = false;
            } else if !escape {
                output.push(character);
            }
        }
        output
    }

    #[test]
    fn folds_multiline_errors_and_trims_single_line_errors() {
        let servers = [server(
            "ghidra",
            McpServerTransport::Stdio,
            McpServerStatus::Failed,
            0,
            Some("MCP error -32000: Connection closed\nstderr: usage: bridge [-h]"),
        )];
        let lines = build_mcp_status_report_lines(&servers)
            .iter()
            .map(|line| strip_sgr(line))
            .collect::<Vec<_>>();
        assert!(lines.iter().all(|line| !line.contains('\n')));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Connection closed stderr: usage: bridge [-h]"))
        );

        let single = [server(
            "ida",
            McpServerTransport::Http,
            McpServerStatus::Failed,
            0,
            Some("  fetch failed  "),
        )];
        assert!(
            build_mcp_status_report_lines(&single)
                .iter()
                .map(|line| strip_sgr(line))
                .any(|line| line.contains("error: fetch failed"))
        );
    }

    #[test]
    fn sorts_statuses_and_builds_actions_and_summary() {
        let servers = [
            server(
                "zeta",
                McpServerTransport::Http,
                McpServerStatus::Connected,
                2,
                None,
            ),
            server(
                "auth",
                McpServerTransport::Sse,
                McpServerStatus::NeedsAuth,
                0,
                None,
            ),
            server(
                "off",
                McpServerTransport::Stdio,
                McpServerStatus::Disabled,
                0,
                None,
            ),
            server(
                "alpha",
                McpServerTransport::Http,
                McpServerStatus::Connected,
                1,
                None,
            ),
        ];
        let plain = build_mcp_status_report_lines(&servers)
            .iter()
            .map(|line| strip_sgr(line))
            .collect::<Vec<_>>();
        let auth = plain
            .iter()
            .position(|line| line.contains("auth"))
            .unwrap_or_default();
        let alpha = plain
            .iter()
            .position(|line| line.contains("alpha"))
            .unwrap_or_default();
        assert!(auth < alpha);
        assert!(
            plain
                .iter()
                .any(|line| line.contains("run /mcp-config login auth"))
        );
        assert!(plain.iter().any(|line| {
            line.contains("2 connected · 1 needs auth · 1 disabled · 3 tools available")
        }));
    }

    #[test]
    fn renders_empty_configuration_message() {
        let plain = build_mcp_status_report_lines(&[])
            .iter()
            .map(|line| strip_sgr(line))
            .collect::<Vec<_>>();
        assert_eq!(plain.len(), 2);
        assert!(plain[1].contains("No MCP servers configured"));
    }
}
