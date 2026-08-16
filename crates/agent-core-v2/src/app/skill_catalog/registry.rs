//! Concrete in-memory skill catalog.
//!
//! Original: `packages/agent-core-v2/src/app/skillCatalog/registry.ts`.

use std::{collections::HashMap, sync::LazyLock};

use regex::{Captures, Regex};
use unicode_segmentation::UnicodeSegmentation;

use crate::_base::utils::xml_escape::{escape_xml_attribute, escape_xml_tags};

use super::{
    parser::skill_argument_names,
    types::{
        SkillCatalogContract, SkillDefinition, SkillSource, SkippedSkill, is_inline_skill_type,
        normalize_skill_name,
    },
};

const LISTING_DESC_MAX: usize = 250;

static INDEXED_ARGUMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$ARGUMENTS\[(\d+)\]").expect("indexed argument regex must compile")
});

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Skill \"{skill_name}\" is not registered")]
pub struct SkillNotFoundError {
    pub skill_name: String,
}

impl SkillNotFoundError {
    pub fn new(skill_name: impl Into<String>) -> Self {
        Self {
            skill_name: skill_name.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RegisterSkillOptions {
    pub replace: bool,
}

#[derive(Clone, Debug, Default)]
pub struct InMemorySkillCatalog {
    by_name: HashMap<String, SkillDefinition>,
    by_plugin_and_name: HashMap<String, SkillDefinition>,
    roots: Vec<String>,
    skipped: Vec<SkippedSkill>,
}

impl InMemorySkillCatalog {
    // Original: InMemorySkillCatalog.registerBuiltinSkill().
    pub fn register_builtin_skill(&mut self, skill: SkillDefinition) {
        let mut builtin = skill;
        builtin.source = SkillSource::Builtin;
        self.register(builtin, RegisterSkillOptions::default());
    }

    // Original: InMemorySkillCatalog.register().
    pub fn register(&mut self, skill: SkillDefinition, options: RegisterSkillOptions) {
        let key = normalize_skill_name(&skill.name);
        if options.replace || !self.by_name.contains_key(&key) {
            if skill.plugin.is_none() {
                self.by_name.insert(key, skill);
                return;
            }
            self.by_name.insert(key, skill.clone());
        }
        self.index_plugin_skill(skill, options);
    }

    pub fn record_skipped(&mut self, skills: &[SkippedSkill]) {
        self.skipped.extend_from_slice(skills);
    }

    pub fn add_roots(&mut self, roots: &[String]) {
        for root in roots {
            if !self.roots.contains(root) {
                self.roots.push(root.clone());
            }
        }
    }

    pub fn get_skill(&self, name: &str) -> Option<&SkillDefinition> {
        self.by_name.get(&normalize_skill_name(name))
    }

    pub fn get_plugin_skill(&self, plugin_id: &str, name: &str) -> Option<&SkillDefinition> {
        self.by_plugin_and_name
            .get(&plugin_skill_key(plugin_id, name))
    }

    // Original: InMemorySkillCatalog.renderSkillPrompt().
    pub fn render_skill_prompt(
        &self,
        skill: &SkillDefinition,
        raw_args: &str,
        session_id: Option<&str>,
    ) -> String {
        self.render_skill_prompt_with_options(skill, raw_args, session_id, true)
    }

    pub fn render_skill_prompt_for_request(
        &self,
        skill: &SkillDefinition,
        raw_args: &str,
        session_id: Option<&str>,
    ) -> String {
        self.render_skill_prompt_with_options(skill, raw_args, session_id, false)
    }

    fn render_skill_prompt_with_options(
        &self,
        skill: &SkillDefinition,
        raw_args: &str,
        session_id: Option<&str>,
        append_unbound_args: bool,
    ) -> String {
        let argument_names = skill_argument_names(&skill.metadata);
        let content = expand_skill_parameters(
            &skill.content,
            raw_args,
            SkillExpandContext {
                skill_dir: &skill.dir,
                session_id,
                argument_names: &argument_names,
            },
            append_unbound_args,
        );
        let Some(plugin) = &skill.plugin else {
            return content;
        };
        let Some(instructions) = plugin.instructions.as_deref().filter(|instructions| {
            instructions
                .chars()
                .any(|character| !is_javascript_whitespace(character))
        }) else {
            return content;
        };
        format!(
            "<kimi-plugin-instructions plugin=\"{}\">\n{}\n</kimi-plugin-instructions>\n\n{}",
            escape_xml_attribute(&plugin.id),
            instructions,
            content
        )
    }

    pub fn list_skills(&self) -> Vec<SkillDefinition> {
        let mut skills = self.by_name.values().cloned().collect::<Vec<_>>();
        skills.sort_by(|left, right| left.name.cmp(&right.name));
        skills
    }

    pub fn list_invocable_skills(&self) -> Vec<SkillDefinition> {
        self.list_skills()
            .into_iter()
            .filter(|skill| {
                skill.metadata.disable_model_invocation != Some(true)
                    && is_inline_skill_type(skill.metadata.kind.as_deref())
            })
            .collect()
    }

    pub fn get_skill_roots(&self) -> Vec<String> {
        self.roots.clone()
    }

    pub fn get_skipped_by_policy(&self) -> Vec<SkippedSkill> {
        self.skipped.clone()
    }

    pub fn get_kimi_skills_description(&self) -> String {
        let rendered = render_grouped_skills(&self.list_skills(), format_full_skill);
        if rendered.is_empty() {
            "No skills".into()
        } else {
            rendered
        }
    }

    pub fn get_model_skill_listing(&self) -> String {
        let skills = self
            .list_invocable_skills()
            .into_iter()
            .filter(|skill| skill.metadata.is_sub_skill != Some(true))
            .collect::<Vec<_>>();
        let listing = render_grouped_skills(&skills, format_model_skill);
        if listing.is_empty() {
            String::new()
        } else {
            format!("DISREGARD any earlier skill listings. Current available skills:\n{listing}")
        }
    }

    fn index_plugin_skill(&mut self, skill: SkillDefinition, options: RegisterSkillOptions) {
        let Some(plugin) = &skill.plugin else {
            return;
        };
        let key = plugin_skill_key(&plugin.id, &skill.name);
        if options.replace || !self.by_plugin_and_name.contains_key(&key) {
            self.by_plugin_and_name.insert(key, skill);
        }
    }
}

impl SkillCatalogContract for InMemorySkillCatalog {
    fn get_skill(&self, name: &str) -> Option<SkillDefinition> {
        InMemorySkillCatalog::get_skill(self, name).cloned()
    }

    fn get_plugin_skill(&self, plugin_id: &str, name: &str) -> Option<SkillDefinition> {
        InMemorySkillCatalog::get_plugin_skill(self, plugin_id, name).cloned()
    }

    fn render_skill_prompt(
        &self,
        skill: &SkillDefinition,
        raw_args: &str,
        session_id: Option<&str>,
    ) -> String {
        InMemorySkillCatalog::render_skill_prompt(self, skill, raw_args, session_id)
    }

    fn render_skill_prompt_for_request(
        &self,
        skill: &SkillDefinition,
        raw_args: &str,
        session_id: Option<&str>,
    ) -> String {
        InMemorySkillCatalog::render_skill_prompt_for_request(self, skill, raw_args, session_id)
    }

    fn list_skills(&self) -> Vec<SkillDefinition> {
        InMemorySkillCatalog::list_skills(self)
    }

    fn list_invocable_skills(&self) -> Vec<SkillDefinition> {
        InMemorySkillCatalog::list_invocable_skills(self)
    }

    fn get_skill_roots(&self) -> Vec<String> {
        InMemorySkillCatalog::get_skill_roots(self)
    }

    fn get_skipped_by_policy(&self) -> Vec<SkippedSkill> {
        InMemorySkillCatalog::get_skipped_by_policy(self)
    }

    fn get_model_skill_listing(&self) -> String {
        InMemorySkillCatalog::get_model_skill_listing(self)
    }
}

struct SkillExpandContext<'a> {
    skill_dir: &'a str,
    session_id: Option<&'a str>,
    argument_names: &'a [String],
}

fn expand_skill_parameters(
    body: &str,
    raw_args: &str,
    context: SkillExpandContext<'_>,
    append_unbound_args: bool,
) -> String {
    let tokens = tokenize_args(raw_args);
    let mut content = body.to_owned();
    for (index, name) in context.argument_names.iter().enumerate() {
        content = replace_named_argument(
            &content,
            name,
            &escape_xml_tags(tokens.get(index).map_or("", String::as_str)),
        );
    }
    content = INDEXED_ARGUMENT
        .replace_all(&content, |captures: &Captures<'_>| {
            let replacement = captures
                .get(1)
                .and_then(|index| index.as_str().parse::<usize>().ok())
                .and_then(|index| tokens.get(index))
                .map_or("", String::as_str);
            escape_xml_tags(replacement)
        })
        .into_owned();
    content = replace_numeric_arguments(&content, &tokens);
    content = content.replace("$ARGUMENTS", &escape_xml_tags(raw_args));

    let has_argument_placeholder = content != body;
    content = content
        .replace("${KIMI_SKILL_DIR}", context.skill_dir)
        .replace("${KIMI_SESSION_ID}", context.session_id.unwrap_or_default());
    if append_unbound_args && !has_argument_placeholder && !raw_args.is_empty() {
        format!("{content}\n\nARGUMENTS: {}", escape_xml_tags(raw_args))
    } else {
        content
    }
}

fn replace_named_argument(content: &str, name: &str, replacement: &str) -> String {
    let needle = format!("${name}");
    let mut output = String::with_capacity(content.len());
    let mut remaining = content;
    while let Some(index) = remaining.find(&needle) {
        output.push_str(&remaining[..index]);
        let after = &remaining[index + needle.len()..];
        if after
            .chars()
            .next()
            .is_some_and(|next| next == '[' || next.is_ascii_alphanumeric() || next == '_')
        {
            output.push_str(&needle);
        } else {
            output.push_str(replacement);
        }
        remaining = after;
    }
    output.push_str(remaining);
    output
}

fn replace_numeric_arguments(content: &str, tokens: &[String]) -> String {
    let mut output = String::with_capacity(content.len());
    let bytes = content.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] != b'$' || cursor + 1 >= bytes.len() || !bytes[cursor + 1].is_ascii_digit()
        {
            let character = content[cursor..]
                .chars()
                .next()
                .expect("cursor is in bounds");
            output.push(character);
            cursor += character.len_utf8();
            continue;
        }
        let mut end = cursor + 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
            output.push_str(&content[cursor..end]);
        } else {
            let replacement = content[cursor + 1..end]
                .parse::<usize>()
                .ok()
                .and_then(|index| tokens.get(index))
                .map_or("", String::as_str);
            output.push_str(&escape_xml_tags(replacement));
        }
        cursor = end;
    }
    output
}

fn tokenize_args(raw: &str) -> Vec<String> {
    let mut output = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut has_content = false;
    for character in raw.chars() {
        if let Some(expected) = quote {
            if character == expected {
                quote = None;
            } else {
                current.push(character);
                has_content = true;
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
            has_content = true;
        } else if is_javascript_whitespace(character) {
            if has_content {
                output.push(std::mem::take(&mut current));
                has_content = false;
            }
        } else {
            current.push(character);
            has_content = true;
        }
    }
    if has_content {
        output.push(current);
    }
    output
}

fn is_javascript_whitespace(character: char) -> bool {
    character.is_whitespace() || character == '\u{feff}'
}

fn plugin_skill_key(plugin_id: &str, skill_name: &str) -> String {
    format!("{plugin_id}\0{}", normalize_skill_name(skill_name))
}

const SOURCE_GROUPS: &[(SkillSource, &str)] = &[
    (SkillSource::Project, "Project"),
    (SkillSource::User, "User"),
    (SkillSource::Extra, "Extra"),
    (SkillSource::Builtin, "Built-in"),
];

fn render_grouped_skills(
    skills: &[SkillDefinition],
    format: fn(&SkillDefinition) -> Vec<String>,
) -> String {
    let mut lines = Vec::new();
    for (source, label) in SOURCE_GROUPS {
        let group = skills
            .iter()
            .filter(|skill| skill.source == *source)
            .collect::<Vec<_>>();
        if group.is_empty() {
            continue;
        }
        lines.push(format!("### {label}"));
        for skill in group {
            lines.extend(format(skill));
        }
    }
    lines.join("\n")
}

fn format_full_skill(skill: &SkillDefinition) -> Vec<String> {
    vec![
        format!("- {}", skill.name),
        format!("  - Path: {}", skill.path),
        format!("  - Description: {}", skill.description),
    ]
}

fn format_model_skill(skill: &SkillDefinition) -> Vec<String> {
    let mut lines = vec![format!(
        "- {}: {}",
        skill.name,
        truncate(&skill.description, LISTING_DESC_MAX)
    )];
    if let Some(when_to_use) = skill
        .metadata
        .when_to_use
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        lines.push(format!("  When to use: {when_to_use}"));
    }
    lines.push(format!("  Path: {}", skill.path));
    lines
}

fn truncate(value: &str, max: usize) -> String {
    if value.encode_utf16().count() <= max {
        return value.into();
    }
    let mut length = 0;
    let mut result = String::new();
    for segment in value.graphemes(true) {
        let segment_length = segment.encode_utf16().count();
        if length + segment_length > max - 3 {
            break;
        }
        result.push_str(segment);
        length += segment_length;
    }
    result + "..."
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value};

    use super::*;
    use crate::app::skill_catalog::types::{SkillMetadata, SkillPluginContext};

    fn skill(name: &str, source: SkillSource) -> SkillDefinition {
        SkillDefinition {
            name: name.into(),
            description: format!("Description for {name}"),
            path: format!("/{name}/SKILL.md"),
            dir: format!("/{name}"),
            content: "body".into(),
            metadata: SkillMetadata {
                extra: Map::new(),
                ..SkillMetadata::default()
            },
            source,
            plugin: None,
            mermaid: None,
            d2: None,
        }
    }

    #[test]
    fn registration_preserves_first_wins_replace_plugin_and_diagnostics_rules() {
        let mut catalog = InMemorySkillCatalog::default();
        catalog.register(
            skill("Review", SkillSource::User),
            RegisterSkillOptions::default(),
        );
        catalog.register(
            skill("review", SkillSource::Project),
            RegisterSkillOptions::default(),
        );
        assert_eq!(
            catalog.get_skill("REVIEW").unwrap().source,
            SkillSource::User
        );
        catalog.register(
            skill("review", SkillSource::Project),
            RegisterSkillOptions { replace: true },
        );
        assert_eq!(
            catalog.get_skill("review").unwrap().source,
            SkillSource::Project
        );

        let mut plugin = skill("Review", SkillSource::User);
        plugin.plugin = Some(SkillPluginContext {
            id: "plug".into(),
            instructions: None,
        });
        catalog.register(plugin, RegisterSkillOptions::default());
        assert!(catalog.get_plugin_skill("plug", "REVIEW").is_some());

        catalog.add_roots(&["/one".into(), "/one".into(), "/two".into()]);
        catalog.record_skipped(&[SkippedSkill {
            path: "/bad".into(),
            kind: "future".into(),
            reason: "unsupported".into(),
        }]);
        assert_eq!(catalog.get_skill_roots(), ["/one", "/two"]);
        assert_eq!(catalog.get_skipped_by_policy().len(), 1);
    }

    #[test]
    fn prompt_rendering_preserves_replacement_order_escaping_and_plugin_wrapper() {
        let catalog = InMemorySkillCatalog::default();
        let mut value = skill("args", SkillSource::User);
        value.content =
            "$FIRST|$SECOND|$ARGUMENTS[0]|$1|$ARGUMENTS|${KIMI_SKILL_DIR}|${KIMI_SESSION_ID}"
                .into();
        value.metadata.arguments = Some(Value::String("FIRST SECOND".into()));
        value.plugin = Some(SkillPluginContext {
            id: "a&\"b".into(),
            instructions: Some("plugin rules".into()),
        });
        assert_eq!(
            catalog.render_skill_prompt(&value, "'<one>' two", Some("session")),
            "<kimi-plugin-instructions plugin=\"a&amp;&quot;b\">\nplugin rules\n</kimi-plugin-instructions>\n\n&lt;one&gt;|two|&lt;one&gt;|two|'&lt;one&gt;' two|/args|session"
        );

        value.plugin = None;
        value.content = "plain ${KIMI_SKILL_DIR}".into();
        assert_eq!(
            catalog.render_skill_prompt(&value, "<raw>", None),
            "plain /args\n\nARGUMENTS: &lt;raw&gt;"
        );
        assert_eq!(
            catalog.render_skill_prompt_for_request(&value, "<raw>", None),
            "plain /args"
        );
    }

    #[test]
    fn listings_group_filter_and_truncate_like_the_source() {
        let mut catalog = InMemorySkillCatalog::default();
        let mut project = skill("project", SkillSource::Project);
        project.description = "😀".repeat(130);
        project.metadata.when_to_use = Some("when needed".into());
        catalog.register(project, RegisterSkillOptions::default());
        let mut hidden = skill("hidden", SkillSource::User);
        hidden.metadata.disable_model_invocation = Some(true);
        catalog.register(hidden, RegisterSkillOptions::default());
        let mut sub = skill("sub", SkillSource::Builtin);
        sub.metadata.is_sub_skill = Some(true);
        catalog.register(sub, RegisterSkillOptions::default());

        let listing = catalog.get_model_skill_listing();
        assert!(listing.starts_with("DISREGARD any earlier skill listings."));
        assert!(listing.contains("### Project\n- project:"));
        assert!(listing.contains("...\n  When to use: when needed"));
        assert!(!listing.contains("hidden"));
        assert!(!listing.contains("sub"));
        assert!(
            catalog
                .get_kimi_skills_description()
                .contains("### Built-in")
        );
    }
}
