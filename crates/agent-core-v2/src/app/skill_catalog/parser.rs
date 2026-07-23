//! Shared Markdown frontmatter parsing primitives.
//!
//! Original: `packages/agent-core-v2/src/app/skillCatalog/parser.ts`,
//! `parseFrontmatter()`.

use serde_json::{Map, Number, Value};
use yaml_rust::{Yaml, YamlLoader};

const FENCE: &str = "---";

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct FrontmatterError {
    pub message: String,
    #[source]
    cause: Option<yaml_rust::ScanError>,
}

impl FrontmatterError {
    fn missing_closing_fence() -> Self {
        Self {
            message: "Missing closing frontmatter fence".into(),
            cause: None,
        }
    }

    fn yaml(error: yaml_rust::ScanError) -> Self {
        Self {
            message: error.to_string(),
            cause: Some(error),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedFrontmatter {
    pub data: Option<Value>,
    pub body: String,
}

// Original: parseFrontmatter(). Splitting explicitly on `\n` preserves a
// trailing empty line and normalizes CRLF only when a frontmatter fence exists,
// matching JavaScript's split/join behavior.
pub fn parse_frontmatter(text: &str) -> Result<ParsedFrontmatter, FrontmatterError> {
    let lines = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect::<Vec<_>>();
    if lines.first().is_none_or(|line| line.trim() != FENCE) {
        return Ok(ParsedFrontmatter {
            data: None,
            body: text.into(),
        });
    }

    let close = lines
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, line)| (line.trim() == FENCE).then_some(index))
        .ok_or_else(FrontmatterError::missing_closing_fence)?;
    let yaml_text = lines[1..close].join("\n").trim().to_owned();
    let body = lines[close + 1..].join("\n");
    if yaml_text.is_empty() {
        return Ok(ParsedFrontmatter {
            data: Some(Value::Object(Map::new())),
            body,
        });
    }

    let documents = YamlLoader::load_from_str(&yaml_text).map_err(FrontmatterError::yaml)?;
    let yaml = documents.first().unwrap_or(&Yaml::Null);
    let data = match yaml {
        Yaml::Null | Yaml::BadValue => Value::Object(Map::new()),
        yaml => yaml_to_json(yaml),
    };
    Ok(ParsedFrontmatter {
        data: Some(data),
        body,
    })
}

fn yaml_to_json(yaml: &Yaml) -> Value {
    match yaml {
        Yaml::Real(value) => value
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(value.clone())),
        Yaml::Integer(value) => Value::Number((*value).into()),
        Yaml::String(value) => Value::String(value.clone()),
        Yaml::Boolean(value) => Value::Bool(*value),
        Yaml::Array(values) => Value::Array(values.iter().map(yaml_to_json).collect()),
        Yaml::Hash(values) => Value::Object(Map::from_iter(
            values
                .iter()
                .map(|(key, value)| (yaml_key(key), yaml_to_json(value))),
        )),
        Yaml::Alias(value) => Value::Number((*value as u64).into()),
        Yaml::Null | Yaml::BadValue => Value::Null,
    }
}

fn yaml_key(key: &Yaml) -> String {
    match key {
        Yaml::String(value) | Yaml::Real(value) => value.clone(),
        Yaml::Integer(value) => value.to_string(),
        Yaml::Boolean(value) => value.to_string(),
        Yaml::Null | Yaml::BadValue => "null".into(),
        Yaml::Alias(value) => value.to_string(),
        Yaml::Array(_) | Yaml::Hash(_) => format!("{key:?}"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn absent_fence_returns_null_data_and_untouched_text() {
        let text = "body\r\nkeeps endings\r\n";
        assert_eq!(
            parse_frontmatter(text).unwrap(),
            ParsedFrontmatter {
                data: None,
                body: text.into(),
            }
        );
    }

    #[test]
    fn parses_yaml_mapping_lists_scalars_and_normalizes_crlf_body() {
        let parsed = parse_frontmatter(
            "---\r\nname: review\r\ntools:\r\n  - Read\r\n  - Grep\r\nsafe: true\r\ncount: 2\r\n---\r\nPrompt\r\n",
        )
        .unwrap();
        assert_eq!(
            parsed.data,
            Some(json!({
                "name": "review",
                "tools": ["Read", "Grep"],
                "safe": true,
                "count": 2
            }))
        );
        assert_eq!(parsed.body, "Prompt\n");
    }

    #[test]
    fn empty_or_null_yaml_becomes_an_empty_mapping() {
        assert_eq!(
            parse_frontmatter("---\n\n---\nbody").unwrap().data,
            Some(json!({}))
        );
        assert_eq!(
            parse_frontmatter("---\nnull\n---\nbody").unwrap().data,
            Some(json!({}))
        );
    }

    #[test]
    fn reports_missing_fence_and_yaml_scanner_errors() {
        assert_eq!(
            parse_frontmatter("---\nname: test")
                .unwrap_err()
                .to_string(),
            "Missing closing frontmatter fence"
        );
        assert!(parse_frontmatter("---\nname: [\n---\nbody").is_err());
    }
}
