#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpToolName<'a> {
    pub server_name: &'a str,
    pub tool_name: &'a str,
}

/// Original:
///   apps/kimi-code/src/tui/utils/mcp-tool-name.ts
///   decodeMcpToolName()
pub fn decode_mcp_tool_name(name: &str) -> Option<McpToolName<'_>> {
    let qualified = name.strip_prefix("mcp__")?;
    let separator = qualified.find("__")?;
    if separator == 0 || separator + 2 == qualified.len() {
        return None;
    }
    Some(McpToolName {
        server_name: &qualified[..separator],
        tool_name: &qualified[separator + 2..],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_first_server_tool_separator() {
        assert_eq!(
            decode_mcp_tool_name("mcp__github__issues__list"),
            Some(McpToolName {
                server_name: "github",
                tool_name: "issues__list"
            })
        );
    }

    #[test]
    fn rejects_non_mcp_empty_and_hash_truncated_names() {
        for name in [
            "Read",
            "mcp__",
            "mcp____tool",
            "mcp__server",
            "mcp__server__",
        ] {
            assert_eq!(decode_mcp_tool_name(name), None);
        }
    }
}
