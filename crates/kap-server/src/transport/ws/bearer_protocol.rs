pub const WS_BEARER_PROTOCOL_PREFIX: &str = "kimi-code.bearer.";

// Original: packages/kap-server/src/transport/ws/bearerProtocol.ts
pub fn extract_ws_bearer_token(protocol_header: Option<&str>) -> Option<&str> {
    protocol_header?
        .split(',')
        .map(str::trim)
        .find_map(|protocol| {
            protocol
                .strip_prefix(WS_BEARER_PROTOCOL_PREFIX)
                .filter(|token| !token.is_empty())
        })
}

pub fn select_ws_bearer_protocol<'a>(
    protocols: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    protocols
        .into_iter()
        .find(|protocol| protocol.starts_with(WS_BEARER_PROTOCOL_PREFIX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_first_non_empty_bearer_protocol() {
        assert_eq!(extract_ws_bearer_token(None), None);
        assert_eq!(extract_ws_bearer_token(Some("chat, other")), None);
        assert_eq!(extract_ws_bearer_token(Some("kimi-code.bearer.")), None);
        assert_eq!(
            extract_ws_bearer_token(Some("chat, kimi-code.bearer.secret, other")),
            Some("secret")
        );
    }

    #[test]
    fn selects_first_bearer_protocol_without_validating_token() {
        assert_eq!(
            select_ws_bearer_protocol(["chat", "kimi-code.bearer.secret", "other"]),
            Some("kimi-code.bearer.secret")
        );
        assert_eq!(select_ws_bearer_protocol(["chat", "other"]), None);
    }
}
