use std::{
    cmp::Ordering,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
};

use crate::tui::fuzzy::fuzzy_match;

const MAX_FALLBACK_SCAN: usize = 2000;
const MAX_FALLBACK_SUGGESTIONS: usize = 50;

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

type ArgumentCompleter = dyn Fn(&str) -> Option<Vec<AutocompleteItem>> + Send + Sync;
type InputModeReader = dyn Fn() -> InputMode + Send + Sync;

#[derive(Clone)]
pub struct SlashAutocompleteCommand {
    pub metadata: SlashCommandMetadata,
    pub complete_arguments: Option<Arc<ArgumentCompleter>>,
}

impl SlashAutocompleteCommand {
    pub fn new(metadata: SlashCommandMetadata) -> Self {
        Self {
            metadata,
            complete_arguments: None,
        }
    }

    pub fn with_argument_completer(mut self, completer: Arc<ArgumentCompleter>) -> Self {
        self.complete_arguments = Some(completer);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Prompt,
    Bash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionEdit {
    pub lines: Vec<String>,
    pub cursor_line: usize,
    /// Unicode scalar-value column, matching the Rust editor cursor model.
    pub cursor_col: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutocompleteSuggestions {
    pub prefix: String,
    pub items: Vec<AutocompleteItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FsMentionCandidate {
    path: String,
    absolute_path: String,
    is_directory: bool,
}

/// Kimi autocomplete provider for slash commands, paths, and `@` mentions.
///
/// Original: `file-mention-provider.ts`, `FileMentionProvider`.
pub struct FileMentionProvider {
    slash_commands: Vec<SlashAutocompleteCommand>,
    work_dir: PathBuf,
    fd_path: Option<String>,
    additional_dirs: Vec<PathBuf>,
    get_input_mode: Arc<InputModeReader>,
}

impl FileMentionProvider {
    pub fn new(
        slash_commands: Vec<SlashAutocompleteCommand>,
        work_dir: PathBuf,
        fd_path: Option<String>,
        additional_dirs: Vec<PathBuf>,
        get_input_mode: Arc<InputModeReader>,
    ) -> Self {
        let additional_dirs = additional_dirs
            .into_iter()
            .map(|directory| resolved_root(&work_dir, &directory))
            .collect();
        Self {
            slash_commands,
            work_dir,
            fd_path,
            additional_dirs,
            get_input_mode,
        }
    }

    pub async fn get_suggestions(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        force: bool,
        cancelled: Arc<AtomicBool>,
    ) -> Option<AutocompleteSuggestions> {
        if cancelled.load(AtomicOrdering::Relaxed) {
            return None;
        }
        let current_line = lines.get(cursor_line).map_or("", String::as_str);
        let text_before_cursor = char_prefix(current_line, cursor_col);

        if let Some(at_prefix) = extract_at_prefix(text_before_cursor) {
            if let Some(fd_path) = self.fd_path.as_deref()
                && is_executable_fd(fd_path)
                && let Ok(result) = self
                    .get_fd_mention_suggestions(fd_path, at_prefix, Arc::clone(&cancelled))
                    .await
            {
                return result;
            }
            return get_fs_mention_suggestions(
                self.work_dir.clone(),
                self.additional_dirs.clone(),
                at_prefix.to_owned(),
                cancelled,
            )
            .await;
        }

        if should_suppress_leading_whitespace_slash_path(text_before_cursor, force)
            || should_suppress_slash_argument_completion(
                text_before_cursor,
                char_suffix(current_line, cursor_col),
                force,
            )
        {
            return None;
        }

        if !force && text_before_cursor.starts_with('/') && !text_before_cursor.contains(' ') {
            return self.slash_name_suggestions(text_before_cursor);
        }

        if (self.get_input_mode)() == InputMode::Prompt
            && let Some(result) = self.slash_argument_suggestions(text_before_cursor)
        {
            return Some(result);
        }

        let prefix = extract_path_prefix(text_before_cursor, force)?;
        let work_dir = self.work_dir.clone();
        let prefix_for_scan = prefix.to_owned();
        let mut result =
            tokio::task::spawn_blocking(move || get_file_suggestions(&work_dir, &prefix_for_scan))
                .await
                .ok()
                .flatten()?;
        if (self.get_input_mode)() == InputMode::Bash {
            result.items.retain(|item| !is_dot_prefixed_entry(item));
        }
        (!result.items.is_empty()).then_some(result)
    }

    pub fn apply_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> CompletionEdit {
        if (self.get_input_mode)() == InputMode::Bash && prefix.starts_with('/') {
            return apply_path_completion(lines, cursor_line, cursor_col, item, prefix);
        }
        apply_inner_completion(lines, cursor_line, cursor_col, item, prefix)
    }

    fn slash_name_suggestions(&self, text_before_cursor: &str) -> Option<AutocompleteSuggestions> {
        let tokens = text_before_cursor[1..]
            .split_whitespace()
            .filter(|token| !token.is_empty())
            .collect::<Vec<_>>();
        let mut matches = Vec::new();
        for command in &self.slash_commands {
            if let Some(score) = score_tokens(&tokens, &command.metadata.name) {
                matches.push((command, score, false, command.metadata.name.clone()));
                continue;
            }
            let best_alias_score = command
                .metadata
                .aliases
                .iter()
                .filter_map(|alias| score_tokens(&tokens, alias))
                .min_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
            if let Some(score) = best_alias_score {
                matches.push((
                    command,
                    score,
                    true,
                    format!(
                        "{} ({})",
                        command.metadata.name,
                        command.metadata.aliases.join(", ")
                    ),
                ));
            }
        }
        matches.sort_by(|left, right| {
            left.1
                .partial_cmp(&right.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.2.cmp(&right.2))
        });
        let items = matches
            .into_iter()
            .map(|(command, _, _, label)| AutocompleteItem {
                value: command.metadata.name.clone(),
                label,
                description: format_slash_command_description(&command.metadata),
            })
            .collect::<Vec<_>>();
        (!items.is_empty()).then_some(AutocompleteSuggestions {
            prefix: text_before_cursor.to_owned(),
            items,
        })
    }

    fn slash_argument_suggestions(
        &self,
        text_before_cursor: &str,
    ) -> Option<AutocompleteSuggestions> {
        let metadata = self
            .slash_commands
            .iter()
            .map(|command| command.metadata.clone())
            .collect::<Vec<_>>();
        let parsed = parse_slash_argument_context(text_before_cursor, &metadata)?;
        let command = self
            .slash_commands
            .iter()
            .find(|command| command.metadata.name == parsed.command.name)?;
        let items = (command.complete_arguments.as_ref()?)(parsed.argument_prefix)?;
        (!items.is_empty()).then_some(AutocompleteSuggestions {
            prefix: parsed.argument_prefix.to_owned(),
            items,
        })
    }

    async fn get_fd_mention_suggestions(
        &self,
        fd_path: &str,
        at_prefix: &str,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Option<AutocompleteSuggestions>, ()> {
        let query = at_prefix.strip_prefix('@').unwrap_or(at_prefix);
        let mut candidates = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (root, additional) in std::iter::once((&self.work_dir, false))
            .chain(self.additional_dirs.iter().map(|root| (root, true)))
        {
            let found = run_fd(fd_path, root, query, Arc::clone(&cancelled)).await?;
            for (relative, is_directory) in found {
                let absolute = root.join(&relative);
                let absolute_path = normalize_path(&absolute);
                if seen.insert(absolute_path.clone()) {
                    candidates.push(FsMentionCandidate {
                        path: if additional {
                            absolute_path.clone()
                        } else {
                            normalize_path(&relative)
                        },
                        absolute_path,
                        is_directory,
                    });
                }
            }
        }
        if cancelled.load(AtomicOrdering::Relaxed) {
            return Ok(None);
        }
        let items = rank_fs_mention_candidates(candidates, query)
            .into_iter()
            .take(MAX_FALLBACK_SUGGESTIONS)
            .map(to_mention_item)
            .collect::<Vec<_>>();
        Ok((!items.is_empty()).then_some(AutocompleteSuggestions {
            prefix: at_prefix.to_owned(),
            items,
        }))
    }
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

/// Filesystem fallback for `@` completion when `fd` is unavailable.
///
/// Original: `file-mention-provider.ts`, `getFsMentionSuggestions()` and
/// `collectFsMentionCandidates()`. Blocking traversal is isolated from the
/// async runtime, unlike the source's synchronous Node filesystem calls.
pub async fn get_fs_mention_suggestions(
    work_dir: PathBuf,
    additional_dirs: Vec<PathBuf>,
    at_prefix: String,
    cancelled: Arc<AtomicBool>,
) -> Option<AutocompleteSuggestions> {
    if cancelled.load(AtomicOrdering::Relaxed) {
        return None;
    }
    tokio::task::spawn_blocking(move || {
        let query = at_prefix.strip_prefix('@').unwrap_or(&at_prefix);
        let candidates = collect_fs_mention_candidates(&work_dir, &additional_dirs, &cancelled);
        if candidates.is_empty() || cancelled.load(AtomicOrdering::Relaxed) {
            return None;
        }
        let items = rank_fs_mention_candidates(candidates, query)
            .into_iter()
            .take(MAX_FALLBACK_SUGGESTIONS)
            .map(to_mention_item)
            .collect::<Vec<_>>();
        (!items.is_empty()).then_some(AutocompleteSuggestions {
            prefix: at_prefix,
            items,
        })
    })
    .await
    .ok()
    .flatten()
}

fn collect_fs_mention_candidates(
    work_dir: &Path,
    additional_dirs: &[PathBuf],
    cancelled: &AtomicBool,
) -> Vec<FsMentionCandidate> {
    let work_root = resolved_root(work_dir, work_dir);
    let roots = std::iter::once((work_root, false)).chain(
        additional_dirs
            .iter()
            .map(|directory| (resolved_root(work_dir, directory), true)),
    );
    let mut candidates_by_absolute_path = HashMap::new();
    let mut scanned = 0;

    for (root, is_additional_dir) in roots {
        let mut stack = vec![PathBuf::new()];
        while let Some(relative_dir) = stack.pop() {
            if scanned >= MAX_FALLBACK_SCAN || cancelled.load(AtomicOrdering::Relaxed) {
                break;
            }
            let absolute_dir = root.join(&relative_dir);
            let Ok(entries) = std::fs::read_dir(&absolute_dir) else {
                continue;
            };
            for entry in entries.flatten() {
                if scanned >= MAX_FALLBACK_SCAN || cancelled.load(AtomicOrdering::Relaxed) {
                    break;
                }
                if entry.file_name() == ".git" {
                    continue;
                }
                let relative_path = relative_dir.join(entry.file_name());
                let absolute_path = absolute_dir.join(entry.file_name());
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                let is_symlink = file_type.is_symlink();
                let is_directory = file_type.is_dir()
                    || (is_symlink
                        && std::fs::metadata(&absolute_path)
                            .is_ok_and(|metadata| metadata.is_dir()));
                scanned += 1;

                let absolute_normalized = normalize_path(&absolute_path);
                candidates_by_absolute_path
                    .entry(absolute_normalized.clone())
                    .or_insert_with(|| FsMentionCandidate {
                        path: if is_additional_dir {
                            absolute_normalized
                        } else {
                            normalize_path(&relative_path)
                        },
                        absolute_path: normalize_path(&absolute_path),
                        is_directory,
                    });
                if is_directory && !is_symlink {
                    stack.push(relative_path);
                }
            }
        }
    }
    candidates_by_absolute_path.into_values().collect()
}

fn rank_fs_mention_candidates(
    candidates: Vec<FsMentionCandidate>,
    query: &str,
) -> Vec<FsMentionCandidate> {
    let lower_query = query.to_lowercase();
    let mut scored = candidates
        .into_iter()
        .filter_map(|candidate| {
            let score = score_candidate(&candidate, &lower_query);
            (score > 0).then_some((candidate, score))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(left, left_score), (right, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| right.is_directory.cmp(&left.is_directory))
            .then_with(|| left.path.cmp(&right.path))
    });
    scored.into_iter().map(|(candidate, _)| candidate).collect()
}

fn score_candidate(candidate: &FsMentionCandidate, lower_query: &str) -> i32 {
    if lower_query.is_empty() {
        let depth_penalty = candidate.path.matches('/').count() as i32;
        return if candidate.is_directory { 120 } else { 100 } - depth_penalty;
    }
    let lower_path = candidate.path.to_lowercase();
    let lower_base = candidate
        .path
        .rsplit('/')
        .next()
        .unwrap_or(&candidate.path)
        .to_lowercase();
    let mut score = if lower_base == lower_query {
        100
    } else if lower_base.starts_with(lower_query) {
        80
    } else if lower_base.contains(lower_query) {
        50
    } else if lower_path.contains(lower_query) {
        30
    } else {
        0
    };
    if candidate.is_directory && score > 0 {
        score += 10;
    }
    score
}

fn to_mention_item(candidate: FsMentionCandidate) -> AutocompleteItem {
    let value_path = if candidate.is_directory {
        format!("{}/", candidate.path)
    } else {
        candidate.path.clone()
    };
    let value = if value_path.contains(' ') {
        format!("@\"{value_path}\"")
    } else {
        format!("@{value_path}")
    };
    let base = candidate.path.rsplit('/').next().unwrap_or(&candidate.path);
    AutocompleteItem {
        value,
        label: format!("{base}{}", if candidate.is_directory { "/" } else { "" }),
        description: Some(candidate.absolute_path),
    }
}

fn resolved_root(work_dir: &Path, directory: &Path) -> PathBuf {
    let joined = if directory.is_absolute() {
        directory.to_owned()
    } else {
        work_dir.join(directory)
    };
    if joined.is_absolute() {
        joined
    } else {
        std::env::current_dir().map_or(joined.clone(), |current| current.join(joined))
    }
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn apply_inner_completion(
    lines: &[String],
    cursor_line: usize,
    cursor_col: usize,
    item: &AutocompleteItem,
    prefix: &str,
) -> CompletionEdit {
    let current_line = lines.get(cursor_line).map_or("", String::as_str);
    let prefix_start = cursor_col.saturating_sub(prefix.chars().count());
    let before_prefix = char_prefix(current_line, prefix_start);
    let mut after_cursor = char_suffix(current_line, cursor_col);
    let quoted_prefix = prefix.starts_with('"') || prefix.starts_with("@\"");
    if quoted_prefix && item.value.ends_with('"') && after_cursor.starts_with('"') {
        after_cursor = char_suffix(after_cursor, 1);
    }

    let slash_command =
        prefix.starts_with('/') && before_prefix.trim().is_empty() && !prefix[1..].contains('/');
    let (new_line, new_cursor) = if slash_command {
        (
            format!("{before_prefix}/{} {after_cursor}", item.value),
            prefix_start + item.value.chars().count() + 2,
        )
    } else if prefix.starts_with('@') {
        let is_directory = item.label.ends_with('/');
        let suffix = if is_directory { "" } else { " " };
        let cursor_offset = completion_value_cursor_offset(item, is_directory);
        (
            format!("{before_prefix}{}{suffix}{after_cursor}", item.value),
            prefix_start + cursor_offset + suffix.chars().count(),
        )
    } else {
        let is_directory = item.label.ends_with('/');
        let cursor_offset = completion_value_cursor_offset(item, is_directory);
        (
            format!("{before_prefix}{}{after_cursor}", item.value),
            prefix_start + cursor_offset,
        )
    };
    let mut new_lines = lines.to_vec();
    if cursor_line < new_lines.len() {
        new_lines[cursor_line] = new_line;
    } else {
        new_lines.resize(cursor_line, String::new());
        new_lines.push(new_line);
    }
    CompletionEdit {
        lines: new_lines,
        cursor_line,
        cursor_col: new_cursor,
    }
}

fn completion_value_cursor_offset(item: &AutocompleteItem, is_directory: bool) -> usize {
    let length = item.value.chars().count();
    if is_directory && item.value.ends_with('"') {
        length.saturating_sub(1)
    } else {
        length
    }
}

fn extract_path_prefix(text: &str, force: bool) -> Option<&str> {
    if let Some(quoted) = extract_quoted_prefix(text) {
        return Some(quoted);
    }
    let token_start = text
        .char_indices()
        .rev()
        .find_map(|(index, character)| {
            is_path_delimiter(character).then_some(index + character.len_utf8())
        })
        .unwrap_or(0);
    let prefix = &text[token_start..];
    if force
        || prefix.contains('/')
        || prefix.starts_with('.')
        || prefix.starts_with("~/")
        || (prefix.is_empty() && text.ends_with(' '))
    {
        Some(prefix)
    } else {
        None
    }
}

fn extract_quoted_prefix(text: &str) -> Option<&str> {
    let mut open_quote = None;
    for (index, character) in text.char_indices() {
        if character == '"' {
            open_quote = if open_quote.is_some() {
                None
            } else {
                Some(index)
            };
        }
    }
    let quote_start = open_quote?;
    if quote_start > 0 && text[..quote_start].ends_with('@') {
        let at_start = quote_start - '@'.len_utf8();
        if at_start == 0
            || text[..at_start]
                .chars()
                .next_back()
                .is_some_and(is_path_delimiter)
        {
            return Some(&text[at_start..]);
        }
        return None;
    }
    if quote_start == 0
        || text[..quote_start]
            .chars()
            .next_back()
            .is_some_and(is_path_delimiter)
    {
        Some(&text[quote_start..])
    } else {
        None
    }
}

fn get_file_suggestions(work_dir: &Path, prefix: &str) -> Option<AutocompleteSuggestions> {
    let (raw_prefix, is_at_prefix, is_quoted_prefix) = parse_path_prefix(prefix);
    let expanded_prefix = expand_home_path(raw_prefix);
    let root_prefix = matches!(raw_prefix, "" | "./" | "../" | "~" | "~/" | "/");
    let (search_dir, search_prefix) = if root_prefix || ends_with_separator(raw_prefix) {
        let directory = if raw_prefix.starts_with('~') || Path::new(&expanded_prefix).is_absolute()
        {
            PathBuf::from(&expanded_prefix)
        } else {
            work_dir.join(&expanded_prefix)
        };
        (directory, String::new())
    } else {
        let (directory, file) = split_display_path(&expanded_prefix);
        let directory = if raw_prefix.starts_with('~') || Path::new(&expanded_prefix).is_absolute()
        {
            PathBuf::from(directory)
        } else {
            work_dir.join(directory)
        };
        (directory, file.to_owned())
    };

    let entries = std::fs::read_dir(&search_dir).ok()?;
    let mut items = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name
            .to_lowercase()
            .starts_with(&search_prefix.to_lowercase())
        {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let is_directory = file_type.is_dir()
            || (file_type.is_symlink()
                && std::fs::metadata(entry.path()).is_ok_and(|metadata| metadata.is_dir()));
        let mut display_path = completion_display_path(raw_prefix, &name);
        if is_directory {
            display_path.push('/');
        }
        let value =
            build_completion_value(&display_path, is_directory, is_at_prefix, is_quoted_prefix);
        items.push(AutocompleteItem {
            value,
            label: format!("{name}{}", if is_directory { "/" } else { "" }),
            description: Some(normalize_path(&entry.path())),
        });
    }
    items.sort_by(|left, right| {
        right
            .label
            .ends_with('/')
            .cmp(&left.label.ends_with('/'))
            .then_with(|| left.label.to_lowercase().cmp(&right.label.to_lowercase()))
    });
    (!items.is_empty()).then_some(AutocompleteSuggestions {
        prefix: prefix.to_owned(),
        items,
    })
}

fn parse_path_prefix(prefix: &str) -> (&str, bool, bool) {
    if let Some(raw) = prefix.strip_prefix("@\"") {
        (raw, true, true)
    } else if let Some(raw) = prefix.strip_prefix('"') {
        (raw, false, true)
    } else if let Some(raw) = prefix.strip_prefix('@') {
        (raw, true, false)
    } else {
        (prefix, false, false)
    }
}

fn expand_home_path(path: &str) -> String {
    if path == "~" {
        return dirs::home_dir().map_or_else(|| path.to_owned(), |home| normalize_path(&home));
    }
    if let Some(relative) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        let mut expanded = normalize_path(&home.join(relative));
        if path.ends_with('/') && !expanded.ends_with('/') {
            expanded.push('/');
        }
        return expanded;
    }
    path.to_owned()
}

fn split_display_path(path: &str) -> (&str, &str) {
    match path.rfind(['/', '\\']) {
        Some(0) => (&path[..1], &path[1..]),
        Some(index) => (&path[..index], &path[index + 1..]),
        None => (".", path),
    }
}

fn ends_with_separator(path: &str) -> bool {
    path.ends_with('/') || path.ends_with('\\')
}

fn completion_display_path(raw_prefix: &str, name: &str) -> String {
    if ends_with_separator(raw_prefix) {
        format!("{raw_prefix}{name}")
    } else if let Some(index) = raw_prefix.rfind(['/', '\\']) {
        format!("{}{name}", &raw_prefix[..=index])
    } else if raw_prefix == "~" {
        format!("~/{name}")
    } else {
        name.to_owned()
    }
}

fn build_completion_value(
    path: &str,
    _is_directory: bool,
    is_at_prefix: bool,
    is_quoted_prefix: bool,
) -> String {
    let prefix = if is_at_prefix { "@" } else { "" };
    if is_quoted_prefix || path.contains(' ') {
        format!("{prefix}\"{path}\"")
    } else {
        format!("{prefix}{path}")
    }
}

fn is_executable_fd(fd_path: &str) -> bool {
    if !fd_path.contains('/') && !fd_path.contains('\\') {
        return true;
    }
    let Ok(metadata) = std::fs::metadata(fd_path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

async fn run_fd(
    fd_path: &str,
    root: &Path,
    query: &str,
    cancelled: Arc<AtomicBool>,
) -> Result<Vec<(PathBuf, bool)>, ()> {
    use std::process::Stdio;

    if cancelled.load(AtomicOrdering::Relaxed) {
        return Ok(Vec::new());
    }
    let mut command = tokio::process::Command::new(fd_path);
    command
        .arg("--base-directory")
        .arg(root)
        .args([
            "--max-results",
            "50",
            "--type",
            "f",
            "--type",
            "d",
            "--follow",
            "--hidden",
            "--exclude",
            ".git",
            "--exclude",
            ".git/*",
            "--exclude",
            ".git/**",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if query.contains('/') {
        command.arg("--full-path");
    }
    if !query.is_empty() {
        command.arg(build_fd_path_query(query));
    }
    let child = command.stdout(Stdio::piped()).spawn().map_err(|_| ())?;
    let output = child.wait_with_output();
    tokio::pin!(output);
    let output = loop {
        tokio::select! {
            result = &mut output => break result.map_err(|_| ())?,
            () = tokio::time::sleep(std::time::Duration::from_millis(20)) => {
                if cancelled.load(AtomicOrdering::Relaxed) {
                    return Ok(Vec::new());
                }
            }
        }
    };
    if !output.status.success() || output.stdout.is_empty() {
        return Ok(Vec::new());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();
    for line in stdout.lines().filter(|line| !line.is_empty()) {
        let display = line.replace('\\', "/");
        let normalized = display.trim_end_matches('/');
        if normalized == ".git" || normalized.starts_with(".git/") || normalized.contains("/.git/")
        {
            continue;
        }
        let path = PathBuf::from(normalized);
        let is_directory = display.ends_with('/') || root.join(&path).is_dir();
        results.push((path, is_directory));
    }
    Ok(results)
}

fn build_fd_path_query(query: &str) -> String {
    let normalized = query.replace('\\', "/");
    if !normalized.contains('/') {
        return normalized;
    }
    let trailing = normalized.ends_with('/');
    let segments = normalized
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(regex::escape)
        .collect::<Vec<_>>();
    if segments.is_empty() {
        return normalized;
    }
    let mut pattern = segments.join(r"[\\/]");
    if trailing {
        pattern.push_str(r"[\\/]");
    }
    pattern
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
    use std::{fs, path::Path};

    use super::*;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "kimi-file-mention-{label}-{}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&path).expect("test directory should be created");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    async fn suggestions(
        work_dir: &Path,
        additional_dirs: &[PathBuf],
        prefix: &str,
    ) -> Option<AutocompleteSuggestions> {
        get_fs_mention_suggestions(
            work_dir.to_owned(),
            additional_dirs.to_vec(),
            prefix.to_owned(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
    }

    fn command(name: &str, aliases: &[&str]) -> SlashCommandMetadata {
        SlashCommandMetadata {
            name: name.to_owned(),
            aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
            description: None,
            argument_hint: None,
        }
    }

    fn provider(
        commands: Vec<SlashAutocompleteCommand>,
        work_dir: &Path,
        mode: InputMode,
    ) -> FileMentionProvider {
        FileMentionProvider::new(
            commands,
            work_dir.to_owned(),
            None,
            Vec::new(),
            Arc::new(move || mode),
        )
    }

    fn active() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
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

    #[tokio::test]
    async fn fallback_finds_nested_files_folders_and_excludes_git() {
        let work = TestDir::new("nested");
        fs::create_dir_all(work.path().join("src/components"))
            .expect("nested directory should be created");
        fs::write(work.path().join("src/components/Button.tsx"), "export {};")
            .expect("source file should be written");
        fs::create_dir(work.path().join(".git")).expect("git directory should be created");
        fs::write(work.path().join(".git/config"), "secret").expect("git file should be written");

        let result = suggestions(work.path(), &[], "@but")
            .await
            .expect("button should match");
        assert_eq!(result.prefix, "@but");
        assert!(
            result
                .items
                .iter()
                .any(|item| item.value == "@src/components/Button.tsx")
        );

        let all = suggestions(work.path(), &[], "@")
            .await
            .expect("root entries should be returned");
        assert!(all.items.iter().any(|item| item.value == "@src/"));
        assert!(
            all.items
                .iter()
                .all(|item| !item.value.starts_with("@.git"))
        );
    }

    #[tokio::test]
    async fn fallback_uses_absolute_additional_paths_and_deduplicates_overlap() {
        let work = TestDir::new("roots");
        let additional = TestDir::new("additional");
        fs::create_dir_all(additional.path().join("src"))
            .expect("additional source directory should be created");
        fs::write(additional.path().join("src/Additional.ts"), "export {};")
            .expect("additional source should be written");

        let result = suggestions(work.path(), &[additional.path().to_owned()], "@add")
            .await
            .expect("additional file should match");
        let expected = format!(
            "@{}",
            normalize_path(&additional.path().join("src/Additional.ts"))
        );
        assert!(result.items.iter().any(|item| item.value == expected));

        let overlap = work.path().join("extra/src");
        fs::create_dir_all(&overlap).expect("overlap directory should be created");
        fs::write(overlap.join("Overlap.ts"), "export {};")
            .expect("overlap file should be written");
        let extra_root = work.path().join("extra");
        let result = suggestions(work.path(), &[extra_root], "@overlap")
            .await
            .expect("overlap file should match");
        let absolute = normalize_path(&overlap.join("Overlap.ts"));
        assert_eq!(
            result
                .items
                .iter()
                .filter(|item| item.description.as_deref() == Some(&absolute))
                .count(),
            1
        );
        assert!(
            result
                .items
                .iter()
                .any(|item| item.value == "@extra/src/Overlap.ts")
        );
    }

    #[tokio::test]
    async fn fallback_quotes_spaces_ranks_directories_and_limits_results() {
        let work = TestDir::new("ranking");
        fs::create_dir(work.path().join("my folder")).expect("spaced directory should be created");
        fs::write(work.path().join("my-file.txt"), "file")
            .expect("matching file should be written");
        for index in 0..60 {
            fs::write(work.path().join(format!("item-{index:02}.txt")), "file")
                .expect("bulk file should be written");
        }

        let spaced = suggestions(work.path(), &[], "@my")
            .await
            .expect("spaced directory should match");
        assert_eq!(spaced.items[0].value, "@\"my folder/\"");
        assert_eq!(spaced.items[0].label, "my folder/");

        let limited = suggestions(work.path(), &[], "@item")
            .await
            .expect("bulk files should match");
        assert_eq!(limited.items.len(), MAX_FALLBACK_SUGGESTIONS);
    }

    #[tokio::test]
    async fn fallback_honors_cancellation_before_scanning() {
        let work = TestDir::new("cancelled");
        fs::write(work.path().join("README.md"), "readme").expect("readme should be written");
        let cancelled = Arc::new(AtomicBool::new(true));
        assert!(
            get_fs_mention_suggestions(
                work.path().to_owned(),
                Vec::new(),
                "@read".to_owned(),
                cancelled,
            )
            .await
            .is_none()
        );
    }

    #[tokio::test]
    async fn provider_searches_primary_names_aliases_and_descriptions() {
        let work = TestDir::new("provider-slash");
        let mut new = command("new", &["clear"]);
        new.description = Some("Start a fresh session".to_owned());
        let mut help = command("help", &["h", "?"]);
        help.description = Some("Show help".to_owned());
        help.argument_hint = Some("[topic]".to_owned());
        let commands = vec![
            SlashAutocompleteCommand::new(new),
            SlashAutocompleteCommand::new(command("skill:lark-calendar", &[])),
            SlashAutocompleteCommand::new(help),
        ];
        let provider = provider(commands, work.path(), InputMode::Prompt);

        let clear = provider
            .get_suggestions(&["/clear".to_owned()], 0, 6, false, active())
            .await
            .expect("alias should match");
        assert_eq!(clear.items[0].value, "new");
        assert_eq!(clear.items[0].label, "new (clear)");

        let all = provider
            .get_suggestions(&["/".to_owned()], 0, 1, false, active())
            .await
            .expect("bare slash should list commands");
        assert!(all.items.iter().all(|item| !item.label.contains('(')));
        let help = all
            .items
            .iter()
            .find(|item| item.value == "help")
            .expect("help should be listed");
        assert_eq!(help.description.as_deref(), Some("[topic] — Show help"));
    }

    #[tokio::test]
    async fn provider_completes_arguments_only_in_prompt_mode() {
        let work = TestDir::new("provider-args");
        let command = SlashAutocompleteCommand::new(command("goal", &["g"]))
            .with_argument_completer(Arc::new(|prefix| {
                prefix
                    .is_empty()
                    .then(|| vec![AutocompleteItem::new("status", "status")])
            }));
        let prompt = provider(vec![command.clone()], work.path(), InputMode::Prompt);
        let result = prompt
            .get_suggestions(&["/g ".to_owned()], 0, 3, false, active())
            .await
            .expect("alias arguments should complete");
        assert_eq!(result.prefix, "");
        assert_eq!(result.items[0].value, "status");

        let bash = provider(vec![command], work.path(), InputMode::Bash);
        assert!(
            bash.get_suggestions(&["/goal ".to_owned()], 0, 6, true, active())
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn provider_prioritizes_mentions_inside_slash_text_and_falls_back_without_fd() {
        let work = TestDir::new("provider-mention");
        fs::write(work.path().join("README.md"), "readme").expect("readme should be written");
        let provider = FileMentionProvider::new(
            vec![SlashAutocompleteCommand::new(command("goal", &[]))],
            work.path().to_owned(),
            Some(normalize_path(&work.path().join("missing-fd"))),
            Vec::new(),
            Arc::new(|| InputMode::Prompt),
        );
        let line = "/goal Fix the @checkout docs";
        let cursor = "/goal Fix the @".chars().count();
        let result = provider
            .get_suggestions(&[line.to_owned()], 0, cursor, false, active())
            .await
            .expect("mention should take priority");
        assert!(result.items.iter().any(|item| item.value == "@README.md"));
        assert!(
            provider
                .get_suggestions(&["email@example".to_owned()], 0, 13, false, active())
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn provider_filters_dot_paths_only_in_bash_mode() {
        let work = TestDir::new("provider-paths");
        fs::create_dir(work.path().join(".hidden")).expect("hidden dir should be created");
        fs::create_dir(work.path().join("visible")).expect("visible dir should be created");
        fs::write(work.path().join(".dotfile"), "").expect("dotfile should be written");
        fs::write(work.path().join("normal.txt"), "").expect("normal file should be written");

        let bash = provider(Vec::new(), work.path(), InputMode::Bash);
        let bash_result = bash
            .get_suggestions(&[String::new()], 0, 0, true, active())
            .await
            .expect("forced bash paths should complete");
        assert!(
            bash_result
                .items
                .iter()
                .any(|item| item.label == "visible/")
        );
        assert!(
            bash_result
                .items
                .iter()
                .any(|item| item.label == "normal.txt")
        );
        assert!(
            bash_result
                .items
                .iter()
                .all(|item| !item.label.starts_with('.'))
        );

        let prompt = provider(Vec::new(), work.path(), InputMode::Prompt);
        let prompt_result = prompt
            .get_suggestions(&[String::new()], 0, 0, true, active())
            .await
            .expect("forced prompt paths should complete");
        assert!(
            prompt_result
                .items
                .iter()
                .any(|item| item.label == ".hidden/")
        );
        assert!(
            prompt_result
                .items
                .iter()
                .any(|item| item.label == ".dotfile")
        );
    }

    #[test]
    fn provider_applies_command_mention_and_bash_path_completions() {
        let work = TestDir::new("provider-apply");
        let prompt = provider(Vec::new(), work.path(), InputMode::Prompt);
        let help = prompt.apply_completion(
            &["/".to_owned()],
            0,
            1,
            &AutocompleteItem::new("help", "help"),
            "/",
        );
        assert_eq!(help.lines, ["/help "]);

        let file = prompt.apply_completion(
            &["hey @read".to_owned()],
            0,
            9,
            &AutocompleteItem::new("@README.md", "README.md"),
            "@read",
        );
        assert_eq!(file.lines, ["hey @README.md "]);
        let directory = prompt.apply_completion(
            &["hey @sr".to_owned()],
            0,
            7,
            &AutocompleteItem::new("@src/", "src/"),
            "@sr",
        );
        assert_eq!(directory.lines, ["hey @src/"]);

        let bash = provider(Vec::new(), work.path(), InputMode::Bash);
        let path = bash.apply_completion(
            &["cd /App".to_owned()],
            0,
            7,
            &AutocompleteItem::new("/Applications/", "Applications/"),
            "/App",
        );
        assert_eq!(path.lines, ["cd /Applications/"]);
    }

    #[test]
    fn fd_helpers_preserve_bare_commands_and_scoped_queries() {
        assert!(is_executable_fd("fd"));
        assert_eq!(build_fd_path_query("src/button"), r"src[\\/]button");
        assert_eq!(
            build_fd_path_query("src/components/"),
            r"src[\\/]components[\\/]"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fallback_does_not_recurse_into_symlinked_directories() {
        use std::os::unix::fs::symlink;

        let work = TestDir::new("symlink");
        fs::write(work.path().join("target.txt"), "target").expect("target should be written");
        symlink(".", work.path().join("current")).expect("symlink should be created");
        let result = suggestions(work.path(), &[], "@target")
            .await
            .expect("target should match");
        assert!(result.items.iter().any(|item| item.value == "@target.txt"));
        assert!(
            result
                .items
                .iter()
                .all(|item| !item.value.starts_with("@current/"))
        );
    }
}
