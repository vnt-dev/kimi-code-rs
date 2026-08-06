//! Pure Markdown agent-file parsing.
//!
//! Original: `packages/agent-core-v2/src/app/agentFileCatalog/agentFile.ts`.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

use crate::app::skill_catalog::{FrontmatterError, parse_frontmatter};

use super::types::{AgentFileDefinition, AgentFileSource};

static AGENT_NAME_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z0-9]+(?:-[a-z0-9]+)*$").expect("agent name regex must compile")
});

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct AgentFileParseError {
    pub message: String,
    #[source]
    cause: Option<FrontmatterError>,
}

impl AgentFileParseError {
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

pub struct ParseAgentFileOptions<'a> {
    pub path: &'a str,
    pub source: AgentFileSource,
    pub text: &'a str,
}

// Original: parseAgentFileText().
pub fn parse_agent_file_text(
    options: ParseAgentFileOptions<'_>,
) -> Result<AgentFileDefinition, AgentFileParseError> {
    let parsed = parse_frontmatter(options.text)
        .map_err(|error| AgentFileParseError::frontmatter(options.path, error))?;
    let Some(frontmatter) = parsed.data else {
        return Err(AgentFileParseError::new(format!(
            "Missing frontmatter in {}",
            options.path
        )));
    };
    let Some(frontmatter) = frontmatter.as_object() else {
        return Err(AgentFileParseError::new(format!(
            "Frontmatter in {} must be a mapping at the top level",
            options.path
        )));
    };

    let name_field = frontmatter.get("name");
    if name_field.is_some_and(|value| !value.is_null() && !value.is_string()) {
        return Err(field_error("name", options.path, "a non-empty string"));
    }
    let name = non_empty_string(name_field)
        .or_else(|| derive_name_from_path(options.path))
        .ok_or_else(|| {
            AgentFileParseError::new(format!(
                "Missing required frontmatter field \"name\" in {}",
                options.path
            ))
        })?;
    if !AGENT_NAME_PATTERN.is_match(&name) {
        return Err(AgentFileParseError::new(format!(
            "Invalid agent name \"{name}\" in {}: expected kebab-case (e.g. \"code-reviewer\")",
            options.path
        )));
    }

    let description = required_non_empty_string(frontmatter, "description", options.path)?;
    let is_override = parse_boolean(frontmatter.get("override"), "override", options.path)?;
    let raw_tools = parse_string_list(frontmatter.get("tools"), "tools", options.path)?;
    let tools = unrestricted_wildcard(raw_tools);
    let disallowed_tools = parse_string_list(
        frontmatter.get("disallowedTools"),
        "disallowedTools",
        options.path,
    )?;
    let raw_subagents = parse_string_list(frontmatter.get("subagents"), "subagents", options.path)?;
    let subagents = unrestricted_wildcard(raw_subagents);

    let prompt = parsed.body.trim().to_owned();
    if prompt.is_empty() {
        return Err(AgentFileParseError::new(format!(
            "Missing prompt body in {}",
            options.path
        )));
    }

    Ok(AgentFileDefinition {
        name,
        description,
        when_to_use: non_empty_string(frontmatter.get("whenToUse")),
        is_override,
        tools,
        disallowed_tools,
        subagents,
        model: non_empty_string(frontmatter.get("model")),
        prompt,
        path: options.path.into(),
        source: options.source,
    })
}

fn parse_boolean(
    value: Option<&Value>,
    field: &str,
    path: &str,
) -> Result<bool, AgentFileParseError> {
    match value {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(field_error(field, path, "a boolean")),
    }
}

fn parse_string_list(
    value: Option<&Value>,
    field: &str,
    path: &str,
) -> Result<Option<Vec<String>>, AgentFileParseError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect(),
        )),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| field_error(field, path, "a list of non-empty strings"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(field_error(
            field,
            path,
            "a comma-separated string or a list of strings",
        )),
    }
}

fn required_non_empty_string(
    frontmatter: &Map<String, Value>,
    field: &str,
    path: &str,
) -> Result<String, AgentFileParseError> {
    let value = frontmatter.get(field);
    if value.is_some_and(|value| !value.is_null() && !value.is_string()) {
        return Err(field_error(field, path, "a non-empty string"));
    }
    non_empty_string(value).ok_or_else(|| {
        AgentFileParseError::new(format!(
            "Missing required frontmatter field \"{field}\" in {path}"
        ))
    })
}

fn field_error(field: &str, path: &str, expected: &str) -> AgentFileParseError {
    AgentFileParseError::new(format!(
        "Frontmatter field \"{field}\" in {path} must be {expected}"
    ))
}

fn derive_name_from_path(path: &str) -> Option<String> {
    let base = path.replace('\\', "/");
    let base = base.rsplit('/').next().unwrap_or_default();
    let name = match base.rsplit_once('.') {
        Some((name, _)) => name,
        None => base,
    };
    (!name.is_empty()).then(|| name.into())
}

fn non_empty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn unrestricted_wildcard(value: Option<Vec<String>>) -> Option<Vec<String>> {
    match value {
        Some(values) if values.as_slice() == ["*"] => None,
        value => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(path: &str, text: &str) -> Result<AgentFileDefinition, AgentFileParseError> {
        parse_agent_file_text(ParseAgentFileOptions {
            path,
            source: AgentFileSource::Project,
            text,
        })
    }

    #[test]
    fn parses_fields_lists_and_trimmed_prompt() {
        let definition = parse(
            "/repo/code-reviewer.md",
            "---\nname: code-reviewer\ndescription: Reviews code\nwhenToUse: Before merging\nmodel: fast-model\noverride: true\ntools: Read, Grep\ndisallowedTools:\n  - Bash\nsubagents:\n  - explore\n---\n\nReview carefully.\n",
        )
        .unwrap();
        assert_eq!(definition.name, "code-reviewer");
        assert_eq!(definition.when_to_use.as_deref(), Some("Before merging"));
        assert_eq!(definition.model.as_deref(), Some("fast-model"));
        assert!(definition.is_override);
        assert_eq!(definition.tools, Some(vec!["Read".into(), "Grep".into()]));
        assert_eq!(definition.disallowed_tools, Some(vec!["Bash".into()]));
        assert_eq!(definition.subagents, Some(vec!["explore".into()]));
        assert_eq!(definition.prompt, "Review carefully.");
    }

    #[test]
    fn derives_name_from_windows_path_and_treats_lone_wildcards_as_unrestricted() {
        let definition = parse(
            r"C:\agents\explore.md",
            "---\ndescription: Explore\ntools: '*'\nsubagents: '*'\n---\nExplore.",
        )
        .unwrap();
        assert_eq!(definition.name, "explore");
        assert_eq!(definition.tools, None);
        assert_eq!(definition.subagents, None);
    }

    #[test]
    fn rejects_missing_or_non_mapping_frontmatter_and_empty_body() {
        assert_eq!(
            parse("agent.md", "body").unwrap_err().to_string(),
            "Missing frontmatter in agent.md"
        );
        assert_eq!(
            parse("agent.md", "---\n- item\n---\nbody")
                .unwrap_err()
                .to_string(),
            "Frontmatter in agent.md must be a mapping at the top level"
        );
        assert_eq!(
            parse("agent.md", "---\ndescription: test\n---\n  ")
                .unwrap_err()
                .to_string(),
            "Missing prompt body in agent.md"
        );
    }

    #[test]
    fn rejects_invalid_names_and_field_types_with_source_messages() {
        assert!(
            parse(
                "agent.md",
                "---\nname: Not Valid\ndescription: test\n---\nbody"
            )
            .unwrap_err()
            .to_string()
            .contains("expected kebab-case")
        );
        assert_eq!(
            parse(
                "agent.md",
                "---\ndescription: test\noverride: yes\n---\nbody"
            )
            .unwrap_err()
            .to_string(),
            "Frontmatter field \"override\" in agent.md must be a boolean"
        );
        assert_eq!(
            parse(
                "agent.md",
                "---\ndescription: test\ntools: [Read, 3]\n---\nbody"
            )
            .unwrap_err()
            .to_string(),
            "Frontmatter field \"tools\" in agent.md must be a list of non-empty strings"
        );
    }
}
