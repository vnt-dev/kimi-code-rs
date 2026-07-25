use super::rate_limit::{AUTH_RATE_LIMIT_CODE, AUTH_RATE_LIMIT_MSG, AuthFailureLimiter};
use crate::services::auth::{AuthTokenService, CredentialValidator, PasswordError};

pub const AUTH_ERROR_CODE: i64 = 40_101;
pub const AUTH_ERROR_MSG: &str = "Unauthorized";
pub const REDACTED: &str = "[redacted]";
const BEARER_PREFIX: &str = "Bearer ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthDecision {
    Bypassed,
    Authorized {
        redact_authorization: bool,
    },
    Rejected {
        status: u16,
        code: i64,
        message: &'static str,
        redact_authorization: bool,
    },
}

pub fn decode_request_path(raw_url: &str) -> Option<String> {
    let path = raw_url.split('?').next().unwrap_or(raw_url);
    percent_decode(path)
}

// Original: auth.ts, defaultIsBypassed().
pub fn is_auth_bypassed(method: &str, raw_url: &str) -> bool {
    if method == "OPTIONS" {
        return true;
    }
    let Some(path) = decode_request_path(raw_url) else {
        return false;
    };
    if method == "GET" && path == "/api/v1/healthz" {
        return true;
    }
    !path.starts_with("/api/") && path != "/openapi.json" && path != "/asyncapi.json"
}

pub fn extract_bearer(header: Option<&str>) -> Option<&str> {
    header?
        .strip_prefix(BEARER_PREFIX)
        .filter(|token| !token.is_empty())
}

pub async fn authorize_request(
    auth_token_service: &AuthTokenService,
    credential_validator: Option<&CredentialValidator>,
    limiter: Option<&AuthFailureLimiter>,
    method: &str,
    raw_url: &str,
    remote_ip: &str,
    authorization: Option<&str>,
) -> Result<AuthDecision, PasswordError> {
    if limiter.is_some_and(|limiter| limiter.is_banned(remote_ip)) {
        return Ok(AuthDecision::Rejected {
            status: 429,
            code: AUTH_RATE_LIMIT_CODE,
            message: AUTH_RATE_LIMIT_MSG,
            redact_authorization: authorization.is_some(),
        });
    }
    let token = extract_bearer(authorization);
    if is_auth_bypassed(method, raw_url) {
        return Ok(AuthDecision::Bypassed);
    }
    let Some(token) = token else {
        if let Some(limiter) = limiter {
            limiter.record_failure(remote_ip);
        }
        return Ok(unauthorized(authorization.is_some()));
    };
    let valid = match credential_validator {
        Some(validator) => validator.is_valid(token).await?,
        None => auth_token_service.is_valid(token).await?,
    };
    if valid {
        Ok(AuthDecision::Authorized {
            redact_authorization: true,
        })
    } else {
        if let Some(limiter) = limiter {
            limiter.record_failure(remote_ip);
        }
        Ok(unauthorized(true))
    }
}

fn unauthorized(redact_authorization: bool) -> AuthDecision {
    AuthDecision::Rejected {
        status: 401,
        code: AUTH_ERROR_CODE,
        message: AUTH_ERROR_MSG,
        redact_authorization,
    }
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex(bytes.get(index + 1).copied()?)?;
            let low = hex(bytes.get(index + 2).copied()?)?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns_bypass_policy_with_decoded_router_path() {
        assert!(is_auth_bypassed("OPTIONS", "/api/v1/sessions"));
        assert!(is_auth_bypassed("GET", "/api/v1/healthz"));
        assert!(is_auth_bypassed("GET", "/%61pi/v1/healthz"));
        assert!(is_auth_bypassed("GET", "/index.html"));
        assert!(!is_auth_bypassed("GET", "/api/v1/sessions"));
        assert!(!is_auth_bypassed("GET", "/%61%70%69/v1/sessions"));
        assert!(!is_auth_bypassed("GET", "/%6fpenapi.json"));
        assert!(!is_auth_bypassed("GET", "/bad%zz"));
    }

    #[test]
    fn bearer_parsing_is_exact_and_case_sensitive() {
        assert_eq!(extract_bearer(Some("Bearer token")), Some("token"));
        assert_eq!(extract_bearer(Some("Bearer ")), None);
        assert_eq!(extract_bearer(Some("bearer token")), None);
        assert_eq!(extract_bearer(None), None);
    }
}
