use serde_json::{Map, Value};

const IMPORTED_BADGE: &str = "[imported]";
const IMPORTED_FLAG_KEY: &str = "imported_from_kimi_cli";

/// Original: `migration/badge.ts`, `isImportedSession()`.
pub fn is_imported_session(metadata: Option<&Map<String, Value>>) -> bool {
    metadata
        .and_then(|metadata| metadata.get(IMPORTED_FLAG_KEY))
        .is_some_and(|value| value == &Value::Bool(true))
}

/// Original: `migration/badge.ts`, `formatSessionLabel()`.
pub fn format_session_label(title: &str, metadata: Option<&Map<String, Value>>) -> String {
    if is_imported_session(metadata) {
        format!("{IMPORTED_BADGE} {title}")
    } else {
        title.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::*;

    #[test]
    fn requires_the_imported_flag_to_be_exactly_boolean_true() {
        assert!(!is_imported_session(None));
        for value in [json!(false), json!(1), json!("true"), Value::Null] {
            let metadata = Map::from_iter([(IMPORTED_FLAG_KEY.to_owned(), value)]);
            assert!(!is_imported_session(Some(&metadata)));
        }
        let metadata = Map::from_iter([(IMPORTED_FLAG_KEY.to_owned(), json!(true))]);
        assert!(is_imported_session(Some(&metadata)));
    }

    #[test]
    fn prefixes_only_imported_session_labels() {
        let metadata = Map::from_iter([(IMPORTED_FLAG_KEY.to_owned(), json!(true))]);
        assert_eq!(
            format_session_label("Legacy chat", Some(&metadata)),
            "[imported] Legacy chat"
        );
        assert_eq!(format_session_label("Current chat", None), "Current chat");
    }
}
