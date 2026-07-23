//! Outbound PII cleaning for telemetry properties.
//!
//! Original: `packages/agent-core-v2/src/app/telemetry/privacy.ts`.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

const REDACTED_PATH: &str = "<REDACTED: user-file-path>";
const NODE_MODULES_MARKER: &str = "node_modules/";

static LABELED_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (
            Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b").unwrap(),
            "<REDACTED: Email>",
        ),
        (
            Regex::new(r#"(?i)https?://[^\s\"'<>]+"#).unwrap(),
            "<REDACTED: URL>",
        ),
        (
            Regex::new(r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{5,}\b")
                .unwrap(),
            "<REDACTED: JWT>",
        ),
        (
            Regex::new(r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{20,}\b").unwrap(),
            "<REDACTED: GitHub Token>",
        ),
        (
            Regex::new(r"\bgithub_pat_[A-Za-z0-9_]{20,}\b").unwrap(),
            "<REDACTED: GitHub Token>",
        ),
        (
            Regex::new(r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b").unwrap(),
            "<REDACTED: Slack Token>",
        ),
        (
            Regex::new(r"\b(?:sk|pk|ak)-[A-Za-z0-9_-]{16,}\b").unwrap(),
            "<REDACTED: API Key>",
        ),
    ]
});

static POSIX_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:/[\w.~+-]+){2,}/?").unwrap());
static WINDOWS_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Za-z]:\\(?:[\w.~ -]+\\?){2,}").unwrap());

// Original: cleanTelemetryString(). Replacement order is significant: URLs
// and token formats are labeled before generic absolute-path redaction.
pub fn clean_telemetry_string(value: &str) -> String {
    let mut output = value.to_owned();
    for (pattern, label) in LABELED_PATTERNS.iter() {
        output = pattern.replace_all(&output, *label).into_owned();
    }
    output = WINDOWS_PATH
        .replace_all(&output, REDACTED_PATH)
        .into_owned();
    POSIX_PATH
        .replace_all(&output, |captures: &regex::Captures<'_>| {
            let matched = captures.get(0).unwrap().as_str();
            matched.find(NODE_MODULES_MARKER).map_or_else(
                || REDACTED_PATH.to_owned(),
                |index| matched[index..].to_owned(),
            )
        })
        .into_owned()
}

// Original: cleanTelemetryProperties(). Only direct string property values are
// cleaned; non-strings and nested values retain their original representation.
pub fn clean_telemetry_properties(properties: &Map<String, Value>) -> Map<String, Value> {
    properties
        .iter()
        .map(|(key, value)| {
            let value = value
                .as_str()
                .map(clean_telemetry_string)
                .map(Value::String)
                .unwrap_or_else(|| value.clone());
            (key.clone(), value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn labels_emails_urls_and_common_token_formats() {
        let input = concat!(
            "user@example.com https://example.test/private ",
            "eyJabcdefghijk.abcdefghijk.abcdef ",
            "ghp_abcdefghijklmnopqrstuvwxyz ",
            "github_pat_abcdefghijklmnopqrstuvwxyz ",
            "xoxb-abcdefghijk ",
            "sk-abcdefghijklmnop"
        );
        assert_eq!(
            clean_telemetry_string(input),
            concat!(
                "<REDACTED: Email> <REDACTED: URL> ",
                "<REDACTED: JWT> <REDACTED: GitHub Token> ",
                "<REDACTED: GitHub Token> <REDACTED: Slack Token> ",
                "<REDACTED: API Key>"
            )
        );
    }

    #[test]
    fn redacts_absolute_paths_but_keeps_node_modules_tail() {
        assert_eq!(
            clean_telemetry_string("/home/alice/project/file.rs"),
            REDACTED_PATH
        );
        assert_eq!(
            clean_telemetry_string("/home/alice/project/node_modules/pkg/index.js"),
            "node_modules/pkg/index.js"
        );
        assert_eq!(
            clean_telemetry_string(r"C:\Users\Alice\project\file.rs"),
            REDACTED_PATH
        );
    }

    #[test]
    fn cleans_only_direct_string_properties() {
        let properties = json!({
            "message": "see /home/alice/private.txt",
            "count": 2,
            "nested": {"path": "/home/alice/private.txt"}
        });
        assert_eq!(
            clean_telemetry_properties(properties.as_object().unwrap()),
            json!({
                "message": "see <REDACTED: user-file-path>",
                "count": 2,
                "nested": {"path": "/home/alice/private.txt"}
            })
            .as_object()
            .unwrap()
            .clone()
        );
    }
}
