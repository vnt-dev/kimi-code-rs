pub const HSTS_VALUE: &str = "max-age=31536000";
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob:; font-src 'self' data:; form-action 'self'; base-uri 'none'; \
frame-ancestors 'self'";

// Original: securityHeaders.ts, createSecurityHeadersHook().
pub fn security_headers(tls: bool) -> Vec<(&'static str, &'static str)> {
    let mut headers = vec![
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
        ("Content-Security-Policy", CONTENT_SECURITY_POLICY),
    ];
    if tls {
        headers.push(("Strict-Transport-Security", HSTS_VALUE));
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_scripts_strict_and_allows_required_inline_styles() {
        assert!(CONTENT_SECURITY_POLICY.contains("style-src 'self' 'unsafe-inline'"));
        let effective_script = CONTENT_SECURITY_POLICY
            .split(';')
            .find(|directive| directive.trim_start().starts_with("script-src"))
            .or_else(|| {
                CONTENT_SECURITY_POLICY
                    .split(';')
                    .find(|directive| directive.trim_start().starts_with("default-src"))
            })
            .unwrap();
        assert!(!effective_script.contains("'unsafe-inline'"));
        assert!(!effective_script.contains("'unsafe-eval'"));
        assert!(!effective_script.contains("data:"));
    }

    #[test]
    fn emits_hsts_only_for_tls() {
        assert!(
            security_headers(false)
                .iter()
                .all(|(name, _)| *name != "Strict-Transport-Security")
        );
        assert!(security_headers(true).contains(&("Strict-Transport-Security", HSTS_VALUE)));
    }
}
