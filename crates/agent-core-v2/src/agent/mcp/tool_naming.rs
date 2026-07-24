//! MCP tool-name qualification.
//!
//! Original: `agent/mcp/tool-naming.ts`.

const MCP_NAME_PREFIX: &str = "mcp__";
const MCP_NAME_SEPARATOR: &str = "__";
const MAX_QUALIFIED_LENGTH: usize = 64;

// Original: sanitizeMcpNamePart().
pub fn sanitize_mcp_name_part(part: &str) -> String {
    let mut output = String::with_capacity(part.len());
    let mut previous_underscore = false;
    for character in part.chars() {
        let next = if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
            character
        } else {
            '_'
        };
        if next == '_' && previous_underscore {
            continue;
        }
        output.push(next);
        previous_underscore = next == '_';
    }
    output
}

// Original: qualifyMcpToolName(). Sanitization leaves ASCII only, so Rust byte
// length and JavaScript UTF-16 length are identical at the truncation boundary.
pub fn qualify_mcp_tool_name(server_name: &str, tool_name: &str) -> String {
    let full = format!(
        "{MCP_NAME_PREFIX}{}{MCP_NAME_SEPARATOR}{}",
        sanitize_mcp_name_part(server_name),
        sanitize_mcp_name_part(tool_name)
    );
    if full.len() <= MAX_QUALIFIED_LENGTH {
        return full;
    }
    let hash = stable_hash8(&full);
    let head_length = MAX_QUALIFIED_LENGTH - hash.len() - 1;
    format!("{}_{}", &full[..head_length], hash)
}

// Original: stableHash8(). Inputs are ASCII after sanitization, so iterating
// bytes is exactly equivalent to the source's codePointAt loop.
fn stable_hash8(input: &str) -> String {
    let mut hash = 0x811c_9dc5_u32;
    for byte in input.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_and_qualifies_like_the_source() {
        assert_eq!(sanitize_mcp_name_part("server name///α"), "server_name_");
        assert_eq!(
            qualify_mcp_tool_name("server name", "read/file"),
            "mcp__server_name__read_file"
        );
    }

    #[test]
    fn long_names_keep_a_stable_eight_hex_suffix() {
        let name = qualify_mcp_tool_name(&"server".repeat(20), &"tool".repeat(20));
        assert_eq!(name.len(), MAX_QUALIFIED_LENGTH);
        assert_eq!(
            name,
            qualify_mcp_tool_name(&"server".repeat(20), &"tool".repeat(20))
        );
        assert!(name.rsplit_once('_').is_some_and(|(_, hash)| {
            hash.len() == 8 && hash.chars().all(|character| character.is_ascii_hexdigit())
        }));
    }
}
