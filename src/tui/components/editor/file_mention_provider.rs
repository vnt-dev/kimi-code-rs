use crate::tui::fuzzy::fuzzy_match;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutocompleteItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

impl AutocompleteItem {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommandMetadata {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionEdit {
    pub lines: Vec<String>,
    pub cursor_line: usize,
    /// Unicode scalar-value column, matching the Rust editor cursor model.
    pub cursor_col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashArgumentContext<'a> {
    pub command: &'a SlashCommandMetadata,
    pub argument_prefix: &'a str,
}

/// Returns the active `@` token after the last path delimiter.
///
/// Original: `file-mention-provider.ts`, `extractAtPrefix()`.
pub fn extract_at_prefix(text: &str) -> Option<&str> {
    let token_start = text
        .char_indices()
        .rev()
        .find_map(|(index, character)| {
            is_path_delimiter(character).then_some(index + character.len_utf8())
        })
        .unwrap_or(0);
    text[token_start..]
        .starts_with('@')
        .then(|| &text[token_start..])
}

/// Replaces a path prefix verbatim without appending a trailing space.
/// Quoted directories keep the cursor before the closing quote.
///
/// Original: `file-mention-provider.ts`, `applyPathCompletion()`.
pub fn apply_path_completion(
    lines: &[String],
    cursor_line: usize,
    cursor_col: usize,
    item: &AutocompleteItem,
    prefix: &str,
) -> CompletionEdit {
    let current_line = lines.get(cursor_line).map_or("", String::as_str);
    let prefix_chars = prefix.chars().count();
    let prefix_start = cursor_col.saturating_sub(prefix_chars);
    let before_prefix = char_prefix(current_line, prefix_start);
    let after_cursor = char_suffix(current_line, cursor_col);
    let new_line = format!("{before_prefix}{}{after_cursor}", item.value);
    let mut new_lines = lines.to_vec();
    if cursor_line < new_lines.len() {
        new_lines[cursor_line] = new_line;
    } else {
        new_lines.resize(cursor_line, String::new());
        new_lines.push(new_line);
    }
    let is_directory = item.label.ends_with('/');
    let has_trailing_quote = item.value.ends_with('"');
    let value_chars = item.value.chars().count();
    let cursor_offset = if is_directory && has_trailing_quote {
        value_chars.saturating_sub(1)
    } else {
        value_chars
    };
    CompletionEdit {
        lines: new_lines,
        cursor_line,
        cursor_col: prefix_start + cursor_offset,
    }
}

pub fn is_dot_prefixed_entry(item: &AutocompleteItem) -> bool {
    item.label.trim_end_matches('/').starts_with('.')
}

pub fn parse_slash_argument_context<'a>(
    text_before_cursor: &'a str,
    slash_commands: &'a [SlashCommandMetadata],
) -> Option<SlashArgumentContext<'a>> {
    let command_text = text_before_cursor.strip_prefix('/')?;

    if let Some(space_index) = command_text.find(char::is_whitespace) {
        let command_name = &command_text[..space_index];
        let after_name = &command_text[space_index..];
        let argument_prefix = after_name.trim_start_matches(char::is_whitespace);
        if argument_prefix.chars().any(char::is_whitespace) {
            return None;
        }
        if !text_before_cursor.ends_with(' ') && argument_prefix.is_empty() {
            return None;
        }
        let command = find_slash_command(slash_commands, command_name)?;
        return Some(SlashArgumentContext {
            command,
            argument_prefix,
        });
    }

    let slash_index = command_text.find('/')?;
    let command_name = &command_text[..slash_index];
    if command_name.is_empty() || command_name.chars().any(char::is_whitespace) {
        return None;
    }
    let command = find_slash_command(slash_commands, command_name)?;
    Some(SlashArgumentContext {
        command,
        argument_prefix: &command_text[slash_index..],
    })
}

pub fn find_slash_command<'a>(
    slash_commands: &'a [SlashCommandMetadata],
    command_name: &str,
) -> Option<&'a SlashCommandMetadata> {
    slash_commands.iter().find(|command| {
        command.name == command_name || command.aliases.iter().any(|alias| alias == command_name)
    })
}

pub fn should_suppress_leading_whitespace_slash_path(
    text_before_cursor: &str,
    force: bool,
) -> bool {
    !force
        && !text_before_cursor.starts_with('/')
        && text_before_cursor.trim_start().starts_with('/')
}

pub fn should_suppress_slash_argument_completion(
    text_before_cursor: &str,
    text_after_cursor: &str,
    force: bool,
) -> bool {
    !force
        && text_before_cursor.starts_with('/')
        && text_before_cursor.contains(' ')
        && !text_after_cursor.trim_start().is_empty()
}

/// All tokens must fuzzy-match; lower scores are better.
pub fn score_tokens(tokens: &[&str], text: &str) -> Option<f64> {
    let mut score = 0.0;
    for token in tokens {
        let result = fuzzy_match(token, text);
        if !result.matches {
            return None;
        }
        score += result.score;
    }
    Some(score)
}

pub fn format_slash_command_description(command: &SlashCommandMetadata) -> Option<String> {
    let description = command.description.as_deref().unwrap_or("");
    let full = match command.argument_hint.as_deref() {
        Some(argument_hint) if !description.is_empty() => {
            format!("{argument_hint} — {description}")
        }
        Some(argument_hint) => argument_hint.to_owned(),
        None => description.to_owned(),
    };
    (!full.is_empty()).then_some(full)
}

fn is_path_delimiter(character: char) -> bool {
    matches!(character, ' ' | '\t' | '"' | '\'' | '=')
}

fn char_prefix(text: &str, count: usize) -> &str {
    let byte_index = text
        .char_indices()
        .nth(count)
        .map_or(text.len(), |(index, _)| index);
    &text[..byte_index]
}

fn char_suffix(text: &str, start: usize) -> &str {
    let byte_index = text
        .char_indices()
        .nth(start)
        .map_or(text.len(), |(index, _)| index);
    &text[byte_index..]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str, aliases: &[&str]) -> SlashCommandMetadata {
        SlashCommandMetadata {
            name: name.to_owned(),
            aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
            description: None,
            argument_hint: None,
        }
    }

    #[test]
    fn extracts_mentions_only_at_delimited_token_start() {
        assert_eq!(extract_at_prefix("@read"), Some("@read"));
        assert_eq!(extract_at_prefix("fix @src/lib"), Some("@src/lib"));
        assert_eq!(extract_at_prefix("key=@docs"), Some("@docs"));
        assert_eq!(extract_at_prefix("\"文件 @路径"), Some("@路径"));
        assert_eq!(extract_at_prefix("email@example.com"), None);
        assert_eq!(extract_at_prefix("slash/@src"), None);
        assert_eq!(extract_at_prefix(""), None);
    }

    #[test]
    fn applies_bash_path_completion_and_preserves_cursor_rules() {
        let directory = AutocompleteItem::new("/Applications/", "Applications/");
        let result = apply_path_completion(&["/".to_owned()], 0, 1, &directory, "/");
        assert_eq!(result.lines, ["/Applications/"]);
        assert_eq!(result.cursor_col, "/Applications/".chars().count());

        let result = apply_path_completion(&["cd /App".to_owned()], 0, 7, &directory, "/App");
        assert_eq!(result.lines, ["cd /Applications/"]);
        assert_eq!(result.cursor_col, "cd /Applications/".chars().count());

        let quoted = AutocompleteItem::new("\"/tmp/My Dir/\"", "My Dir/");
        let result = apply_path_completion(&["cd /tmp/My".to_owned()], 0, 10, &quoted, "/tmp/My");
        assert_eq!(result.lines, ["cd \"/tmp/My Dir/\""]);
        assert_eq!(result.cursor_col, "cd \"/tmp/My Dir/".chars().count());
    }

    #[test]
    fn path_completion_uses_unicode_character_columns() {
        let item = AutocompleteItem::new("目录/", "目录/");
        let result = apply_path_completion(&["执行 cd /目".to_owned()], 0, 8, &item, "/目");
        assert_eq!(result.lines, ["执行 cd 目录/"]);
        assert_eq!(result.cursor_col, 9);
    }

    #[test]
    fn parses_whitespace_path_like_and_alias_argument_contexts() {
        let commands = [command("goal", &["g"]), command("add-dir", &[])];
        let context = parse_slash_argument_context("/goal status", &commands)
            .expect("goal context should parse");
        assert_eq!(context.command.name, "goal");
        assert_eq!(context.argument_prefix, "status");

        let alias =
            parse_slash_argument_context("/g ", &commands).expect("alias context should parse");
        assert_eq!(alias.command.name, "goal");
        assert_eq!(alias.argument_prefix, "");

        let path = parse_slash_argument_context("/add-dir/tmp/shared", &commands)
            .expect("path-like context should parse");
        assert_eq!(path.argument_prefix, "/tmp/shared");
        assert!(parse_slash_argument_context("/goal many words", &commands).is_none());
        assert!(parse_slash_argument_context("/unknown x", &commands).is_none());
    }

    #[test]
    fn applies_slash_completion_suppression_guards() {
        assert!(should_suppress_leading_whitespace_slash_path(
            "  /tmp", false
        ));
        assert!(!should_suppress_leading_whitespace_slash_path(
            "  /tmp", true
        ));
        assert!(!should_suppress_leading_whitespace_slash_path(
            "/tmp", false
        ));

        assert!(should_suppress_slash_argument_completion(
            "/goal ",
            "existing text",
            false
        ));
        assert!(!should_suppress_slash_argument_completion(
            "/goal ",
            "existing text",
            true
        ));
        assert!(!should_suppress_slash_argument_completion(
            "/goal ", "   ", false
        ));
    }

    #[test]
    fn scores_tokens_formats_descriptions_and_detects_dot_entries() {
        assert!(score_tokens(&["hlp"], "help").is_some());
        assert!(score_tokens(&["missing"], "help").is_none());
        assert_eq!(score_tokens(&[], "help"), Some(0.0));

        let mut metadata = command("goal", &[]);
        metadata.description = Some("Start or manage a goal".to_owned());
        metadata.argument_hint = Some("[status|cancel]".to_owned());
        assert_eq!(
            format_slash_command_description(&metadata).as_deref(),
            Some("[status|cancel] — Start or manage a goal")
        );
        metadata.description = None;
        assert_eq!(
            format_slash_command_description(&metadata).as_deref(),
            Some("[status|cancel]")
        );

        assert!(is_dot_prefixed_entry(&AutocompleteItem::new(
            ".hidden/", ".hidden/"
        )));
        assert!(!is_dot_prefixed_entry(&AutocompleteItem::new(
            "visible/", "visible/"
        )));
    }
}
