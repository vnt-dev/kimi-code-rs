//! Plugin Markdown command parsing and loading.
//!
//! Original: `packages/agent-core-v2/src/app/plugin/commands.ts`.

use std::path::Path;

use serde_json::{Map, Value};
use thiserror::Error;

use crate::app::skill_catalog::{FrontmatterError, parse_frontmatter};

use super::types::PluginCommandDef;

pub struct ParseCommandTextOptions<'a> {
    pub text: &'a str,
    pub command_path: &'a str,
    pub plugin_id: &'a str,
    pub fallback_name: Option<&'a str>,
}

pub struct LoadPluginCommandOptions<'a> {
    pub command_path: &'a str,
    pub plugin_id: &'a str,
    pub fallback_name: Option<&'a str>,
}

#[derive(Debug, Error)]
pub enum ParseCommandError {
    #[error(transparent)]
    Frontmatter(#[from] FrontmatterError),
    #[error("failed to resolve command path: {0}")]
    ResolvePath(#[from] std::io::Error),
}

// Original: commands.ts, parseCommandText().
pub fn parse_command_text(
    input: ParseCommandTextOptions<'_>,
) -> Result<PluginCommandDef, ParseCommandError> {
    let parsed = parse_frontmatter(input.text)?;
    let empty = Map::new();
    let frontmatter = parsed
        .data
        .as_ref()
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    let base_name = input
        .fallback_name
        .map(str::to_owned)
        .unwrap_or_else(|| fallback_name(input.command_path));
    let name = frontmatter
        .get("name")
        .and_then(non_empty_string)
        .unwrap_or(base_name);

    let body = parsed.body.trim().to_owned();
    let description = frontmatter
        .get("description")
        .and_then(non_empty_string)
        .unwrap_or_else(|| description_from_body(&body));

    Ok(PluginCommandDef {
        plugin_id: input.plugin_id.to_owned(),
        name,
        description,
        body,
        path: path_to_string(std::path::absolute(input.command_path)?),
    })
}

// Original: commands.ts, loadPluginCommand(). Both read and parse failures are
// intentionally collapsed to `None`.
pub async fn load_plugin_command(input: LoadPluginCommandOptions<'_>) -> Option<PluginCommandDef> {
    let text = tokio::fs::read_to_string(input.command_path).await.ok()?;
    parse_command_text(ParseCommandTextOptions {
        text: &text,
        command_path: input.command_path,
        plugin_id: input.plugin_id,
        fallback_name: input.fallback_name,
    })
    .ok()
}

// Original: commands.ts, expandCommandArguments().
pub fn expand_command_arguments(body: &str, args: &str) -> String {
    let replaced = body.replace("$ARGUMENTS", args);
    if !body.contains("$ARGUMENTS") && !args.is_empty() {
        return format!("{replaced}\n\nARGUMENTS: {args}");
    }
    replaced
}

fn fallback_name(command_path: &str) -> String {
    let basename = Path::new(command_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command_path);
    basename
        .strip_suffix(".md")
        .or_else(|| basename.strip_suffix(".MD"))
        .or_else(|| {
            basename
                .get(basename.len().saturating_sub(3)..)
                .filter(|suffix| suffix.eq_ignore_ascii_case(".md"))
                .map(|_| &basename[..basename.len() - 3])
        })
        .unwrap_or(basename)
        .to_owned()
}

fn non_empty_string(value: &Value) -> Option<String> {
    let trimmed = value.as_str()?.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

// Original: commands.ts, descriptionFromBody(). JavaScript counts and slices
// UTF-16 code units; conversion is lossy only for the unrepresentable case
// where the 239-unit boundary splits a surrogate pair.
fn description_from_body(body: &str) -> String {
    let Some(first_line) = body.lines().map(str::trim).find(|line| !line.is_empty()) else {
        return "No description provided.".to_owned();
    };
    if first_line.encode_utf16().count() <= 240 {
        return first_line.to_owned();
    }
    let truncated = first_line.encode_utf16().take(239).collect::<Vec<_>>();
    format!("{}…", String::from_utf16_lossy(&truncated))
}

fn path_to_string(path: impl AsRef<Path>) -> String {
    path.as_ref().to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_resolves_command_path() {
        let command = parse_command_text(ParseCommandTextOptions {
            text: "---\nname: deploy\ndescription: ' Deploy safely '\n---\n  run $ARGUMENTS  \n",
            command_path: "commands/fallback.md",
            plugin_id: "demo",
            fallback_name: None,
        })
        .unwrap();
        assert_eq!(command.name, "deploy");
        assert_eq!(command.description, "Deploy safely");
        assert_eq!(command.body, "run $ARGUMENTS");
        assert_eq!(command.plugin_id, "demo");
        assert!(Path::new(&command.path).is_absolute());
        assert!(command.path.ends_with("commands/fallback.md"));
    }

    #[test]
    fn falls_back_to_name_and_body_description() {
        let first_line = "x".repeat(241);
        let command = parse_command_text(ParseCommandTextOptions {
            text: &format!("\n{first_line}\nsecond"),
            command_path: "/plugins/demo/Build.Md",
            plugin_id: "demo",
            fallback_name: None,
        })
        .unwrap();
        assert_eq!(command.name, "Build");
        assert_eq!(command.description, format!("{}…", "x".repeat(239)));

        let overridden = parse_command_text(ParseCommandTextOptions {
            text: "",
            command_path: "/plugins/demo/ignored.md",
            plugin_id: "demo",
            fallback_name: Some("declared"),
        })
        .unwrap();
        assert_eq!(overridden.name, "declared");
        assert_eq!(overridden.description, "No description provided.");
    }

    #[test]
    fn expands_all_placeholders_or_appends_arguments() {
        assert_eq!(
            expand_command_arguments("run $ARGUMENTS then $ARGUMENTS", "now"),
            "run now then now"
        );
        assert_eq!(
            expand_command_arguments("run", "now"),
            "run\n\nARGUMENTS: now"
        );
        assert_eq!(expand_command_arguments("run", ""), "run");
    }

    #[tokio::test]
    async fn async_loader_returns_none_for_read_and_parse_failures() {
        let directory =
            std::env::temp_dir().join(format!("plugin-command-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let valid = directory.join("valid.md");
        let invalid = directory.join("invalid.md");
        tokio::fs::write(&valid, "hello").await.unwrap();
        tokio::fs::write(&invalid, "---\nname: [\n---\nbody")
            .await
            .unwrap();

        assert!(load(&valid).await.is_some());
        assert!(load(&invalid).await.is_none());
        assert!(load(&directory.join("missing.md")).await.is_none());

        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    async fn load(path: &Path) -> Option<PluginCommandDef> {
        load_plugin_command(LoadPluginCommandOptions {
            command_path: path.to_str().unwrap(),
            plugin_id: "demo",
            fallback_name: None,
        })
        .await
    }
}
