/// Build the deep-link URL recognized by the web UI. The bearer token stays
/// in the fragment so browsers do not send it to the server in HTTP requests.
///
/// Original:
///   apps/kimi-code/src/tui/commands/web.ts
///   webSessionUrl()
pub fn web_session_url(origin: &str, session_id: &str, token: Option<&str>) -> String {
    let origin = origin.trim_end_matches('/');
    let session_id = encode_uri_component(session_id);
    let base = format!("{origin}/sessions/{session_id}");
    token.map_or_else(|| base.clone(), |token| format!("{base}#token={token}"))
}

fn encode_uri_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            )
        {
            encoded.push(char::from(byte));
        } else {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_links_under_origin_and_removes_all_trailing_slashes() {
        assert_eq!(
            web_session_url("http://127.0.0.1:58627///", "abc123", None),
            "http://127.0.0.1:58627/sessions/abc123"
        );
    }

    #[test]
    fn uses_javascript_component_encoding_for_session_id() {
        assert_eq!(
            web_session_url("https://example.test", "a/b c!\u{1f63a}", None),
            "https://example.test/sessions/a%2Fb%20c!%F0%9F%98%BA"
        );
        assert_eq!(
            web_session_url("https://example.test", "-_.!~*'()", None),
            "https://example.test/sessions/-_.!~*'()"
        );
    }

    #[test]
    fn carries_unmodified_token_in_fragment_only_when_present() {
        assert_eq!(
            web_session_url("http://127.0.0.1:58627", "abc123", Some("tok-1")),
            "http://127.0.0.1:58627/sessions/abc123#token=tok-1"
        );
        assert_eq!(
            web_session_url("http://127.0.0.1:58627", "abc123", Some("")),
            "http://127.0.0.1:58627/sessions/abc123#token="
        );
    }
}
