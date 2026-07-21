pub const OAUTH_LOGIN_REQUIRED_CODE: &str = "auth.login_required";

/// Original:
///   apps/kimi-code/src/tui/utils/startup.ts
///   combineStartupNotice()
pub fn combine_startup_notice(existing: Option<&str>, next: Option<&str>) -> Option<String> {
    match (existing, next) {
        (Some(existing), Some(next)) => Some(format!("{existing}\n{next}")),
        (Some(existing), None) => Some(existing.to_owned()),
        (None, Some(next)) => Some(next.to_owned()),
        (None, None) => None,
    }
}

/// Rust callers extract an optional structured error code at their SDK or RPC
/// boundary before using this predicate.
///
/// Original:
///   apps/kimi-code/src/tui/utils/startup.ts
///   isOAuthLoginRequiredError()
pub fn is_oauth_login_required_error(code: Option<&str>) -> bool {
    code == Some(OAUTH_LOGIN_REQUIRED_CODE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combines_two_notices_in_original_order() {
        assert_eq!(
            combine_startup_notice(Some("first"), Some("second")),
            Some("first\nsecond".to_owned())
        );
    }

    #[test]
    fn preserves_single_notice_and_absence() {
        assert_eq!(
            combine_startup_notice(Some("existing"), None),
            Some("existing".to_owned())
        );
        assert_eq!(
            combine_startup_notice(None, Some("next")),
            Some("next".to_owned())
        );
        assert_eq!(combine_startup_notice(None, None), None);
    }

    #[test]
    fn matches_only_oauth_login_required_code() {
        assert!(is_oauth_login_required_error(Some("auth.login_required")));
        assert!(!is_oauth_login_required_error(Some("provider.auth_error")));
        assert!(!is_oauth_login_required_error(None));
    }
}
