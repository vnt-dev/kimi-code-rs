//! Shared Markdown frontmatter parsing primitives.
//!
//! Original: `packages/agent-core-v2/src/app/skillCatalog/parser.ts`,
//! `parseFrontmatter()` and `parseSkillText()`.

use std::{path::Path, sync::LazyLock};

use regex::Regex;
use serde_json::{Map, Number, Value};
use yaml_rust::{Yaml, YamlLoader};

use super::types::{SkillDefinition, SkillMetadata, SkillSource, is_supported_skill_type};

const FENCE: &str = "---";

static MERMAID_FLOWCHART: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"```mermaid\r?\n([\s\S]*?)\r?\n```").expect("mermaid fence regex must compile")
});
static D2_FLOWCHART: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"```d2\r?\n([\s\S]*?)\r?\n```").expect("d2 fence regex must compile")
});
static NUMERIC_ARGUMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+$").expect("numeric argument regex must compile"));

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

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct SkillParseError {
    pub message: String,
    #[source]
    cause: Option<FrontmatterError>,
}

impl SkillParseError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cause: None,
        }
    }

    fn frontmatter(path: &str, error: FrontmatterError) -> Self {
        Self {
            message: format!("Invalid frontmatter in {path}: {error}"),
            cause: Some(error),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error(
    "Skill type \"{skill_type}\" is not supported; only \"prompt\", \"inline\", and \"flow\" are supported."
)]
pub struct UnsupportedSkillTypeError {
    pub skill_type: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ParseSkillError {
    #[error(transparent)]
    Parse(#[from] SkillParseError),
    #[error(transparent)]
    Unsupported(#[from] UnsupportedSkillTypeError),
}

pub struct ParseSkillTextOptions<'a> {
    pub skill_md_path: &'a str,
    pub skill_dir_name: &'a str,
    pub source: SkillSource,
    pub text: &'a str,
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

// Original: parseSkillText().
pub fn parse_skill_text(
    options: ParseSkillTextOptions<'_>,
) -> Result<SkillDefinition, ParseSkillError> {
    let is_directory_skill = path_basename(options.skill_md_path) == "SKILL.md";
    let first_line = options
        .text
        .split_once('\n')
        .map_or(options.text, |(line, _)| line)
        .trim();
    if is_directory_skill && first_line != FENCE {
        return Err(SkillParseError::new(format!(
            "Missing frontmatter in {}",
            options.skill_md_path
        ))
        .into());
    }

    let parsed = parse_frontmatter(options.text)
        .map_err(|error| SkillParseError::frontmatter(options.skill_md_path, error))?;
    let frontmatter = parsed.data.unwrap_or_else(|| Value::Object(Map::new()));
    let Some(frontmatter) = frontmatter.as_object() else {
        return Err(SkillParseError::new(format!(
            "Frontmatter in {} must be a mapping at the top level",
            options.skill_md_path
        ))
        .into());
    };

    let metadata = normalize_metadata(frontmatter);
    let invalid_raw_type = metadata.extra.get("type");
    if !is_supported_skill_type(metadata.kind.as_deref()) || invalid_raw_type.is_some() {
        let raw_type = invalid_raw_type
            .or_else(|| frontmatter.get("type"))
            .map(javascript_string)
            .unwrap_or_else(|| "undefined".into());
        return Err(UnsupportedSkillTypeError {
            skill_type: metadata.kind.clone().unwrap_or(raw_type),
        }
        .into());
    }

    if is_directory_skill && (metadata.name.is_none() || metadata.description.is_none()) {
        let field = if metadata.name.is_none() {
            "\"name\""
        } else {
            "\"description\""
        };
        return Err(SkillParseError::new(format!(
            "Missing required frontmatter field {field} in {}",
            options.skill_md_path
        ))
        .into());
    }

    let skill_path = absolute_path(options.skill_md_path);
    let directory = Path::new(&skill_path)
        .parent()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let content = parsed.body.trim().to_owned();
    Ok(SkillDefinition {
        name: metadata
            .name
            .clone()
            .unwrap_or_else(|| options.skill_dir_name.into()),
        description: metadata
            .description
            .clone()
            .unwrap_or_else(|| description_from_body(&content)),
        path: skill_path,
        dir: directory,
        mermaid: parse_mermaid_flowchart(&content),
        d2: parse_d2_flowchart(&content),
        content,
        metadata,
        source: options.source,
        plugin: None,
    })
}

// Original: parseMermaidFlowchart().
pub fn parse_mermaid_flowchart(markdown: &str) -> Option<String> {
    MERMAID_FLOWCHART
        .captures(markdown)
        .and_then(|captures| captures.get(1))
        .map(|capture| capture.as_str().to_owned())
}

// Original: parseD2Flowchart().
pub fn parse_d2_flowchart(markdown: &str) -> Option<String> {
    D2_FLOWCHART
        .captures(markdown)
        .and_then(|captures| captures.get(1))
        .map(|capture| capture.as_str().to_owned())
}

// Original: skillArgumentNames(). Array entries retain their source whitespace,
// matching `Array.filter`; string entries are whitespace-tokenized.
pub fn skill_argument_names(metadata: &SkillMetadata) -> Vec<String> {
    match metadata.arguments.as_ref() {
        Some(Value::String(value)) => value
            .split_whitespace()
            .filter(|name| valid_argument_name(name))
            .map(str::to_owned)
            .collect(),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .filter(|name| valid_argument_name(name))
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn normalize_metadata(raw: &Map<String, Value>) -> SkillMetadata {
    let mut normalized = Map::new();
    for (raw_key, value) in raw {
        let key = match raw_key.as_str() {
            "when-to-use" | "when_to_use" => "whenToUse",
            "disable-model-invocation" | "disable_model_invocation" => "disableModelInvocation",
            key => key,
        };
        normalized.insert(key.into(), value.clone());
    }

    let name = take_non_empty_string(&mut normalized, "name");
    let description = take_non_empty_string(&mut normalized, "description");
    let kind = take_non_empty_string(&mut normalized, "type");
    let when_to_use = take_non_empty_string(&mut normalized, "whenToUse");
    let disable_model_invocation = take_bool(&mut normalized, "disableModelInvocation");
    let is_sub_skill = take_bool(&mut normalized, "isSubSkill");
    let safe = take_bool(&mut normalized, "safe");
    let arguments = normalized.remove("arguments");
    SkillMetadata {
        name,
        description,
        kind,
        when_to_use,
        disable_model_invocation,
        is_sub_skill,
        safe,
        arguments,
        extra: normalized,
    }
}

fn take_non_empty_string(values: &mut Map<String, Value>, key: &str) -> Option<String> {
    let value = values.get(key)?.as_str()?.trim().to_owned();
    if value.is_empty() {
        None
    } else {
        values.remove(key);
        Some(value)
    }
}

fn take_bool(values: &mut Map<String, Value>, key: &str) -> Option<bool> {
    let value = values.get(key)?.as_bool()?;
    values.remove(key);
    Some(value)
}

fn description_from_body(body: &str) -> String {
    let Some(first_line) = body.lines().map(str::trim).find(|line| !line.is_empty()) else {
        return "No description provided.".into();
    };
    let count = first_line.chars().count();
    if count > 240 {
        first_line.chars().take(239).collect::<String>() + "…"
    } else {
        first_line.into()
    }
}

fn valid_argument_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && !NUMERIC_ARGUMENT.is_match(name)
}

fn path_basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or_default()
}

fn absolute_path(path: &str) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| Path::new(path).to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn javascript_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "null".into(),
        value => value.to_string(),
    }
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

    #[test]
    fn parses_directory_skill_metadata_aliases_and_flowcharts() {
        let skill = parse_skill_text(ParseSkillTextOptions {
            skill_md_path: "skills/review/SKILL.md",
            skill_dir_name: "review-dir",
            source: SkillSource::Project,
            text: "---\nname: review\ndescription: Review code\ntype: flow\nwhen-to-use: before merge\ndisable_model_invocation: true\narguments: file mode 123\nfuture: kept\n---\nRun it.\n\n```mermaid\ngraph TD\n```\n\n```d2\na -> b\n```",
        })
        .unwrap();

        assert_eq!(skill.name, "review");
        assert_eq!(skill.metadata.kind.as_deref(), Some("flow"));
        assert_eq!(skill.metadata.when_to_use.as_deref(), Some("before merge"));
        assert_eq!(skill.metadata.disable_model_invocation, Some(true));
        assert_eq!(skill.metadata.extra.get("future"), Some(&json!("kept")));
        assert_eq!(skill_argument_names(&skill.metadata), ["file", "mode"]);
        assert_eq!(skill.mermaid.as_deref(), Some("graph TD"));
        assert_eq!(skill.d2.as_deref(), Some("a -> b"));
        assert!(Path::new(&skill.path).is_absolute());
        assert!(skill.path.ends_with("skills/review/SKILL.md"));
    }

    #[test]
    fn standalone_skill_falls_back_to_directory_name_and_body_description() {
        let skill = parse_skill_text(ParseSkillTextOptions {
            skill_md_path: "skills/standalone.md",
            skill_dir_name: "standalone",
            source: SkillSource::User,
            text: "First useful line.\nSecond line.",
        })
        .unwrap();
        assert_eq!(skill.name, "standalone");
        assert_eq!(skill.description, "First useful line.");
        assert_eq!(skill.metadata, SkillMetadata::default());
    }

    #[test]
    fn directory_skills_require_frontmatter_name_and_description() {
        assert_eq!(
            parse_skill_text(ParseSkillTextOptions {
                skill_md_path: "/skills/test/SKILL.md",
                skill_dir_name: "test",
                source: SkillSource::Extra,
                text: "body",
            })
            .unwrap_err()
            .to_string(),
            "Missing frontmatter in /skills/test/SKILL.md"
        );
        assert_eq!(
            parse_skill_text(ParseSkillTextOptions {
                skill_md_path: "/skills/test/SKILL.md",
                skill_dir_name: "test",
                source: SkillSource::Extra,
                text: "---\ndescription: Test\n---\nbody",
            })
            .unwrap_err()
            .to_string(),
            "Missing required frontmatter field \"name\" in /skills/test/SKILL.md"
        );
    }

    #[test]
    fn unsupported_type_keeps_the_source_error_contract() {
        let error = parse_skill_text(ParseSkillTextOptions {
            skill_md_path: "custom.md",
            skill_dir_name: "custom",
            source: SkillSource::Builtin,
            text: "---\ntype: executable\n---\nbody",
        })
        .unwrap_err();
        assert!(matches!(error, ParseSkillError::Unsupported(_)));
        assert_eq!(
            error.to_string(),
            "Skill type \"executable\" is not supported; only \"prompt\", \"inline\", and \"flow\" are supported."
        );

        let numeric = parse_skill_text(ParseSkillTextOptions {
            skill_md_path: "custom.md",
            skill_dir_name: "custom",
            source: SkillSource::Builtin,
            text: "---\ntype: 7\n---\nbody",
        })
        .unwrap_err();
        assert_eq!(
            numeric.to_string(),
            "Skill type \"7\" is not supported; only \"prompt\", \"inline\", and \"flow\" are supported."
        );
    }
}
