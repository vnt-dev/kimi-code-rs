use crate::oauth::managed_auth::KIMI_CODE_PROVIDER_NAME;

pub const DEFAULT_OAUTH_PROVIDER_NAME: &str = KIMI_CODE_PROVIDER_NAME;
pub const PRODUCT_NAME: &str = "Kimi Code";
pub const LLM_NOT_SET_MESSAGE: &str = "LLM not set, send \"/login\" to login";
pub const NO_ACTIVE_SESSION_MESSAGE: &str = "No active session. Send /login to login.";
pub const CTRL_D_HINT: &str = "Press Ctrl+D again to exit";
pub const CTRL_C_HINT: &str = "Press Ctrl+C again to exit";
pub const MAIN_AGENT_ID: &str = "main";
pub const OAUTH_LOGIN_REQUIRED_STARTUP_NOTICE: &str = "OAuth login expired. Send /login to login.";
pub const EXIT_CONFIRM_WINDOW_MS: u64 = 1_500;
pub const DOUBLE_ESC_WINDOW_MS: u64 = 600;

// Original: `src/tui/constant/kimi-tui.ts`, `isManagedUsageProvider()`.
pub fn is_managed_usage_provider(provider_key: Option<&str>) -> bool {
    provider_key == Some(DEFAULT_OAUTH_PROVIDER_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_usage_requires_the_exact_oauth_provider_key() {
        assert!(is_managed_usage_provider(Some("managed:kimi-code")));
        assert!(!is_managed_usage_provider(Some("kimi-code")));
        assert!(!is_managed_usage_provider(None));
    }
}
