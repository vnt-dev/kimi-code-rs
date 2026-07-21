use std::{
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
