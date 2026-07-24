use std::collections::HashMap;

use kimi_code_protocol::parse_or_generate_request_id;

pub const REQUEST_ID_HEADER: &str = "x-request-id";

// Original: request-id.ts, resolveRequestId().
pub fn resolve_request_id(headers: &HashMap<String, Vec<String>>) -> String {
    parse_or_generate_request_id(
        headers
            .get(REQUEST_ID_HEADER)
            .and_then(|values| values.first())
            .map(String::as_str),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_valid_first_header_and_replaces_invalid_input() {
        let valid = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        assert_eq!(
            resolve_request_id(&HashMap::from([(
                REQUEST_ID_HEADER.into(),
                vec![valid.into(), "ignored".into()]
            )])),
            valid
        );
        let generated = resolve_request_id(&HashMap::from([(
            REQUEST_ID_HEADER.into(),
            vec!["attacker input".into()],
        )]));
        assert_ne!(generated, "attacker input");
        assert_eq!(generated.len(), 26);
    }
}
