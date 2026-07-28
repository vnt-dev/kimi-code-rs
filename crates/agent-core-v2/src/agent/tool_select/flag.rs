//! Progressive tool-disclosure feature flag.
//!
//! Original: `packages/agent-core-v2/src/agent/toolSelect/flag.ts`.

use std::sync::LazyLock;

use crate::app::flag::{FlagDefinitionInput, FlagSurface, register_flag_definition};

pub const TOOL_SELECT_FLAG_ID: &str = "tool-select";
pub const TOOL_SELECT_FLAG_ENV: &str = "KIMI_CODE_EXPERIMENTAL_TOOL_SELECT";

pub static TOOL_SELECT_FLAG: LazyLock<FlagDefinitionInput> = LazyLock::new(|| {
    FlagDefinitionInput {
        id: TOOL_SELECT_FLAG_ID.into(),
        title: "Tool select (progressive tool disclosure)".into(),
        description:
            "Keep MCP tool schemas out of the immutable top-level tools[]; the model loads them on demand via the select_tools tool. Only takes effect on models whose capability catalog declares dynamically loaded tools."
                .into(),
        env: TOOL_SELECT_FLAG_ENV.into(),
        default: false,
        surface: FlagSurface::Core,
    }
});

pub fn register_tool_select_flag() {
    register_flag_definition(TOOL_SELECT_FLAG.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_matches_the_typescript_contribution() {
        assert_eq!(TOOL_SELECT_FLAG.id, TOOL_SELECT_FLAG_ID);
        assert_eq!(TOOL_SELECT_FLAG.env, TOOL_SELECT_FLAG_ENV);
        assert_eq!(
            TOOL_SELECT_FLAG.title,
            "Tool select (progressive tool disclosure)"
        );
        assert!(
            TOOL_SELECT_FLAG
                .description
                .contains("dynamically loaded tools")
        );
        assert!(!TOOL_SELECT_FLAG.default);
        assert_eq!(TOOL_SELECT_FLAG.surface, FlagSurface::Core);
    }
}
