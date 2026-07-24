//! MCP-domain error-code registration.
//!
//! Original: `agent/mcp/errors.ts`.

use std::sync::LazyLock;

use crate::_base::errors::codes::{ErrorDomain, register_error_domain};

pub const MCP_SERVER_NOT_FOUND: &str = "mcp.server_not_found";
pub const MCP_SERVER_DISABLED: &str = "mcp.server_disabled";
pub const MCP_STARTUP_FAILED: &str = "mcp.startup_failed";
pub const MCP_TOOL_NAME_COLLISION: &str = "mcp.tool_name_collision";

pub static MCP_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("MCP_SERVER_NOT_FOUND", MCP_SERVER_NOT_FOUND),
        ("MCP_SERVER_DISABLED", MCP_SERVER_DISABLED),
        ("MCP_STARTUP_FAILED", MCP_STARTUP_FAILED),
        ("MCP_TOOL_NAME_COLLISION", MCP_TOOL_NAME_COLLISION),
    ],
    retryable: &[],
    info: &[],
};

static MCP_ERRORS_REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&MCP_ERRORS).expect("MCP error codes must remain unique");
});

pub fn ensure_mcp_errors_registered() {
    LazyLock::force(&MCP_ERRORS_REGISTERED);
}

#[cfg(test)]
mod tests {
    use crate::_base::errors::codes::is_error_code;

    use super::*;

    #[test]
    fn registers_source_mcp_codes() {
        ensure_mcp_errors_registered();
        for code in [
            MCP_SERVER_NOT_FOUND,
            MCP_SERVER_DISABLED,
            MCP_STARTUP_FAILED,
            MCP_TOOL_NAME_COLLISION,
        ] {
            assert!(is_error_code(code));
        }
    }
}
