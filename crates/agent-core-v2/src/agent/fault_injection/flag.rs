use std::sync::LazyLock;

use crate::app::flag::{FlagDefinitionInput, FlagSurface, register_flag_definition};

pub const FAULT_INJECTION_FLAG_ID: &str = "fault-injection";
pub const FAULT_INJECTION_FLAG_ENV: &str = "KIMI_CODE_EXPERIMENTAL_FAULT_INJECTION";

// Original:
//   packages/agent-core-v2/src/agent/faultInjection/flag.ts
//   faultInjectionFlag
pub static FAULT_INJECTION_FLAG: LazyLock<FlagDefinitionInput> = LazyLock::new(|| {
    FlagDefinitionInput {
        id: FAULT_INJECTION_FLAG_ID.into(),
        title: "Fault injection (LLM request failures)".into(),
        description: "Allow arming a one-shot deterministic provider failure (HTTP 413 body-size or image-format rejection) on the next LLM request, for testing the media-degraded / media-stripped recovery projections over a live channel.".into(),
        env: FAULT_INJECTION_FLAG_ENV.into(),
        default: false,
        surface: FlagSurface::Core,
    }
});

pub fn register_fault_injection_flag() {
    register_flag_definition(FAULT_INJECTION_FLAG.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_preserves_id_env_copy_and_default() {
        assert_eq!(FAULT_INJECTION_FLAG.id, FAULT_INJECTION_FLAG_ID);
        assert_eq!(FAULT_INJECTION_FLAG.env, FAULT_INJECTION_FLAG_ENV);
        assert_eq!(
            FAULT_INJECTION_FLAG.title,
            "Fault injection (LLM request failures)"
        );
        assert!(
            FAULT_INJECTION_FLAG
                .description
                .contains("HTTP 413 body-size or image-format rejection")
        );
        assert!(!FAULT_INJECTION_FLAG.default);
        assert_eq!(FAULT_INJECTION_FLAG.surface, FlagSurface::Core);
    }
}
