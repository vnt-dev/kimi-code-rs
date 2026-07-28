//! Pure filename-search, glob, grep-pattern, and ripgrep JSON helpers.
//!
//! Original: `packages/agent-core-v2/src/session/sessionFs/fsSearch.ts`.

use std::{path::Path, sync::Arc};

use base64::{Engine, engine::general_purpose::STANDARD};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use regex::Regex;
use regress::{Flags as JsRegexFlags, Regex as JsRegex};
use serde::Deserialize;

use super::FsGrepRequest;

#[derive(Clone)]
pub struct GitignoreMatcher {
    lines: Vec<String>,
    matcher: Arc<Gitignore>,
}

impl GitignoreMatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, contents: &str) {
        self.lines.extend(contents.lines().map(str::to_owned));
        self.matcher = Arc::new(build_gitignore(&self.lines));
    }

    pub fn ignores(&self, relative_path: &str) -> bool {
        let relative_path = relative_path.trim_start_matches("./");
        self.matcher
            .matched_path_or_any_parents(
                Path::new(relative_path.trim_end_matches('/')),
                relative_path.ends_with('/'),
            )
            .is_ignore()
    }
}

impl Default for GitignoreMatcher {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            matcher: Arc::new(build_gitignore(&[])),
        }
    }
}

fn build_gitignore(lines: &[String]) -> Gitignore {
    let mut builder = GitignoreBuilder::new(".");
    for line in lines {
        let _ = builder.add_line(None, line);
    }
    builder
        .build()
        .expect("building an in-memory gitignore matcher cannot fail")
}

pub fn compute_fuzzy_score(name: &str, query_lower: &str) -> f64 {
    if query_lower.is_empty() {
        return 0.0;
    }
    let name_lower = name.to_lowercase();
    let mut name_index = 0;
    let mut matched = 0;
    for character in query_lower.chars() {
        let Some(found) = name_lower[name_index..].find(character) else {
            return 0.0;
        };
        matched += 1;
        name_index += found + character.len_utf8();
    }
    if matched == 0 {
        return 0.0;
    }
    let query_length = query_lower.encode_utf16().count();
    let mut score = matched as f64 / query_length as f64;
    if name_lower.starts_with(query_lower) {
        score = (score + 0.2).min(1.0);
    }
    score.clamp(0.0, 1.0)
}

pub fn compute_match_positions(path: &str, query_lower: &str) -> Vec<usize> {
    if query_lower.is_empty() {
        return Vec::new();
    }
    let lower = path.to_lowercase().encode_utf16().collect::<Vec<_>>();
    let mut output = Vec::new();
    let mut position = 0;
    for character in query_lower.chars() {
        let needle = character.to_string().encode_utf16().collect::<Vec<_>>();
        let Some(found) = find_utf16(&lower, &needle, position) else {
            return Vec::new();
        };
        output.push(found);
        position = found + 1;
    }
    output
}

fn find_utf16(haystack: &[u16], needle: &[u16], start: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(start.min(haystack.len()));
    }
    haystack
        .get(start..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|position| start + position)
}

pub fn matches_any_glob(relative_path: &str, globs: &[String]) -> bool {
    globs
        .iter()
        .any(|glob| glob_to_regex(glob).is_ok_and(|expression| expression.is_match(relative_path)))
}

fn glob_to_regex(glob: &str) -> Result<Regex, regex::Error> {
    let mut expression = String::from("^");
    append_glob_body(&mut expression, glob);
    expression.push('$');
    Regex::new(&expression)
}

fn append_glob_body(expression: &mut String, glob: &str) {
    let characters = glob.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if character == '*' && characters.get(index + 1) == Some(&'*') {
            expression.push_str(".*");
            index += 2;
            if characters.get(index) == Some(&'/') {
                index += 1;
            }
        } else if character == '*' {
            expression.push_str("[^/]*");
            index += 1;
        } else if character == '?' {
            expression.push_str("[^/]");
            index += 1;
        } else {
            if matches!(
                character,
                '.' | '+' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\'
            ) {
                expression.push('\\');
            }
            expression.push(character);
            index += 1;
        }
    }
}

pub fn compile_grep_pattern(request: &FsGrepRequest) -> Result<JsRegex, regress::Error> {
    let body = if request.regex {
        request.pattern.clone()
    } else {
        regex::escape(&request.pattern)
    };
    JsRegex::with_flags(
        &body,
        JsRegexFlags {
            icase: !request.case_sensitive,
            ..JsRegexFlags::default()
        },
    )
}

pub fn strip_trailing_newline(value: &str) -> &str {
    value
        .strip_suffix("\r\n")
        .or_else(|| value.strip_suffix('\n'))
        .unwrap_or(value)
}

#[derive(Clone, Debug, Deserialize)]
pub struct RgPathField {
    pub text: Option<String>,
    pub bytes: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RgLinesField {
    pub text: Option<String>,
    pub bytes: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RgSubmatch {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RgJsonData {
    pub path: Option<RgPathField>,
    pub lines: Option<RgLinesField>,
    pub line_number: Option<u64>,
    pub submatches: Option<Vec<RgSubmatch>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RgJsonRecordType {
    Begin,
    End,
    Match,
    Context,
    Summary,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RgJsonRecord {
    #[serde(rename = "type")]
    pub record_type: RgJsonRecordType,
    pub data: Option<RgJsonData>,
}

pub fn rg_path(path: Option<&RgPathField>) -> Option<String> {
    let path = path?;
    let raw = if let Some(text) = path.text.as_ref() {
        text.clone()
    } else {
        String::from_utf8(STANDARD.decode(path.bytes.as_ref()?).ok()?).ok()?
    };
    Some(raw.strip_prefix("./").unwrap_or(&raw).to_owned())
}

pub fn rg_text(lines: Option<&RgLinesField>) -> String {
    let Some(lines) = lines else {
        return String::new();
    };
    if let Some(text) = lines.text.as_ref() {
        return text.clone();
    }
    lines
        .bytes
        .as_ref()
        .and_then(|bytes| STANDARD.decode(bytes).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_score_and_positions_match_source_subsequence_rules() {
        assert_eq!(compute_fuzzy_score("anything", ""), 0.0);
        assert_eq!(compute_fuzzy_score("abc", "az"), 0.0);
        assert_eq!(compute_fuzzy_score("foo-bar", "foo"), 1.0);
        assert_eq!(compute_fuzzy_score("bar-foo", "foo"), 1.0);
        assert_eq!(compute_fuzzy_score("😀", "😀"), 0.7);
        assert_eq!(compute_match_positions("src/foo.ts", "foo"), [4, 5, 6]);
        assert!(compute_match_positions("abc", "ca").is_empty());
        assert_eq!(compute_match_positions("😀foo", "foo"), [2, 3, 4]);
    }

    #[test]
    fn globs_preserve_single_and_recursive_wildcards() {
        assert!(!matches_any_glob("src/a.ts", &["*.ts".into()]));
        assert!(matches_any_glob("a.ts", &["*.ts".into()]));
        assert!(matches_any_glob("src/a.ts", &["**/*.ts".into()]));
        assert!(!matches_any_glob("src/a.js", &["**/*.ts".into()]));

        let mut ignore = GitignoreMatcher::new();
        ignore.add(".git/\ndist/\n*.log\n!important.log\n");
        assert!(ignore.ignores("src/.git/config"));
        assert!(ignore.ignores("dist/x.js"));
        assert!(ignore.ignores("nested/error.log"));
        assert!(!ignore.ignores("important.log"));
    }

    #[test]
    fn grep_compilation_newline_and_rg_fields_match_source() {
        let request = FsGrepRequest {
            pattern: "a.b".into(),
            context_lines: 0,
            ..FsGrepRequest::default()
        };
        let expression = compile_grep_pattern(&request).unwrap();
        assert!(expression.find("aXb").is_none());
        assert!(expression.find("a.b").is_some());
        assert_eq!(strip_trailing_newline("a\r\n"), "a");
        assert_eq!(strip_trailing_newline("a\nb"), "a\nb");
        assert_eq!(
            rg_path(Some(&RgPathField {
                text: Some("./src/a.ts".into()),
                bytes: None
            }))
            .as_deref(),
            Some("src/a.ts")
        );
        assert_eq!(
            rg_path(Some(&RgPathField {
                text: None,
                bytes: Some(STANDARD.encode("src/a.ts"))
            }))
            .as_deref(),
            Some("src/a.ts")
        );
        assert!(rg_path(None).is_none());
    }
}
