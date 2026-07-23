//! Permission rule-subject glob and path matching.
//!
//! Original: `packages/agent-core-v2/src/tool/rule-match.ts`.

use std::collections::HashSet;

use globset::GlobBuilder;

use super::path_access::{PathClass, canonicalize_path};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PermissionPathMatchOptions<'a> {
    pub cwd: Option<&'a str>,
    pub path_class: Option<PathClass>,
    pub home_dir: Option<&'a str>,
    pub case_insensitive_paths: Option<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PathMatchSemantics {
    path_class: PathClass,
}

// Original: globMatch(). Invalid glob syntax is treated as non-matching.
pub fn glob_match(value: &str, pattern: &str, nocase: bool) -> bool {
    if glob_match_once(value, pattern, nocase) {
        return true;
    }

    let normalized_value = strip_leading_dot_slash(value);
    let normalized_pattern = strip_leading_dot_slash(pattern);
    if normalized_value == value && normalized_pattern == pattern {
        return false;
    }
    glob_match_once(normalized_value, normalized_pattern, nocase)
}

fn glob_match_once(value: &str, pattern: &str, nocase: bool) -> bool {
    // MIGRATION-TODO:
    // Original dependency: picomatch extglobs such as `@(a|b)` and `+(ab)`.
    // Temporary behavior: globset preserves stars, globstars, classes, braces,
    // escaping, separators, and case folding, but unsupported extglobs do not match.
    // Completion condition: add a compatible extglob parser or Rust matcher.
    GlobBuilder::new(pattern)
        .case_insensitive(nocase)
        .literal_separator(true)
        .backslash_escape(true)
        .build()
        .is_ok_and(|glob| {
            glob.compile_matcher().is_match(value) && dot_segments_are_explicit(value, pattern)
        })
}

// picomatch's default `dot: false` requires a leading dot to be explicit in
// each path segment. Globset deliberately has no dotfile special case, so
// preserve that safety-relevant behavior after its structural match.
fn dot_segments_are_explicit(value: &str, pattern: &str) -> bool {
    let values = value.split('/').collect::<Vec<_>>();
    let patterns = pattern.split('/').collect::<Vec<_>>();
    dot_segments_are_explicit_from(&values, &patterns)
}

fn dot_segments_are_explicit_from(values: &[&str], patterns: &[&str]) -> bool {
    let Some((pattern, remaining_patterns)) = patterns.split_first() else {
        return values.is_empty();
    };
    if *pattern == "**" {
        return dot_segments_are_explicit_from(values, remaining_patterns)
            || values
                .split_first()
                .is_some_and(|(value, remaining_values)| {
                    !value.starts_with('.')
                        && dot_segments_are_explicit_from(remaining_values, patterns)
                });
    }
    let Some((value, remaining_values)) = values.split_first() else {
        return false;
    };
    (!value.starts_with('.') || pattern_starts_with_literal_dot(pattern))
        && dot_segments_are_explicit_from(remaining_values, remaining_patterns)
}

fn pattern_starts_with_literal_dot(pattern: &str) -> bool {
    pattern.starts_with('.') || pattern.starts_with(r"\.")
}

fn strip_leading_dot_slash(value: &str) -> &str {
    value.strip_prefix("./").unwrap_or(value)
}

// Original: pathGlobMatch().
pub fn path_glob_match(
    value: &str,
    pattern: &str,
    options: PermissionPathMatchOptions<'_>,
) -> bool {
    let semantics = path_match_semantics(value, pattern, options.path_class);
    let nocase = options.case_insensitive_paths.unwrap_or(true);

    if glob_match(value, pattern, nocase) {
        return true;
    }

    path_variants(value, semantics, options)
        .iter()
        .any(|value| {
            path_variants(pattern, semantics, options)
                .iter()
                .any(|pattern| glob_match(value, pattern, nocase))
        })
}

fn path_variants(
    value: &str,
    semantics: PathMatchSemantics,
    options: PermissionPathMatchOptions<'_>,
) -> HashSet<String> {
    let mut variants = HashSet::new();
    add_path_variant(&mut variants, value, semantics.path_class);
    add_path_variant(
        &mut variants,
        strip_leading_dot_path(value, semantics.path_class),
        semantics.path_class,
    );

    if let Some(canonical) = canonicalize_path_pattern(value, semantics, options) {
        add_path_variant(&mut variants, &canonical, semantics.path_class);
    }
    variants
}

fn canonicalize_path_pattern(
    value: &str,
    semantics: PathMatchSemantics,
    options: PermissionPathMatchOptions<'_>,
) -> Option<String> {
    let expanded = expand_user_path(value, semantics.path_class, options.home_dir);
    let cwd = options
        .cwd
        .map(str::to_owned)
        .or_else(|| default_cwd_for_path(&expanded));
    canonicalize_path(&expanded, cwd.as_deref()?, semantics.path_class).ok()
}

fn expand_user_path(value: &str, path_class: PathClass, home_dir: Option<&str>) -> String {
    let Some(home_dir) = home_dir else {
        return value.to_owned();
    };
    if value == "~" {
        return home_dir.to_owned();
    }
    let windows_suffix = if path_class == PathClass::Win32 {
        value.strip_prefix("~\\")
    } else {
        None
    };
    let suffix = value.strip_prefix("~/").or(windows_suffix);
    suffix.map_or_else(
        || value.to_owned(),
        |suffix| format!("{}/{}", home_dir.trim_end_matches(['/', '\\']), suffix),
    )
}

fn default_cwd_for_path(value: &str) -> Option<String> {
    let normalized = value.replace('\\', "/");
    if normalized.starts_with('/') {
        Some("/".into())
    } else if normalized.len() >= 3
        && normalized.as_bytes()[0].is_ascii_alphabetic()
        && &normalized.as_bytes()[1..3] == b":/"
    {
        Some(normalized[..3].to_owned())
    } else {
        None
    }
}

fn path_match_semantics(
    value: &str,
    pattern: &str,
    path_class: Option<PathClass>,
) -> PathMatchSemantics {
    let inferred = [value, pattern].iter().any(|candidate| {
        let bytes = candidate.as_bytes();
        (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
            || candidate.starts_with("\\\\")
            || candidate.contains('\\')
    });
    PathMatchSemantics {
        path_class: path_class.unwrap_or(if inferred {
            PathClass::Win32
        } else {
            PathClass::Posix
        }),
    }
}

fn add_path_variant(variants: &mut HashSet<String>, value: &str, path_class: PathClass) {
    variants.insert(value.to_owned());
    if path_class == PathClass::Win32 {
        variants.insert(value.replace('\\', "/"));
    }
}

fn strip_leading_dot_path(value: &str, path_class: PathClass) -> &str {
    value.strip_prefix("./").unwrap_or_else(|| {
        if path_class == PathClass::Win32 {
            value.strip_prefix(".\\").unwrap_or(value)
        } else {
            value
        }
    })
}

pub fn literal_rule_pattern(tool_name: &str, subject: &str) -> String {
    format!("{tool_name}({})", escape_rule_subject_literal(subject))
}

pub fn escape_rule_subject_literal(subject: &str) -> String {
    let mut escaped = String::with_capacity(subject.len());
    for character in subject.chars() {
        if matches!(
            character,
            '\\' | '*' | '?' | '[' | ']' | '{' | '}' | '(' | ')' | '!' | '+' | '@' | '|'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

pub fn matches_glob_rule_subject(rule_args: &str, subject: &str) -> bool {
    match_rule_subjects(rule_args, &[subject], |pattern, value| {
        glob_match(value, pattern, false)
    })
}

pub fn matches_path_rule_subject(
    rule_args: &str,
    subject: &str,
    options: PermissionPathMatchOptions<'_>,
) -> bool {
    match_rule_subjects(rule_args, &[subject], |pattern, value| {
        path_glob_match(value, pattern, options)
    })
}

fn match_rule_subjects(
    rule_args: &str,
    subjects: &[&str],
    matches_positive_pattern: impl Fn(&str, &str) -> bool,
) -> bool {
    if rule_args.is_empty() {
        return true;
    }
    let (negated, positive_pattern) = rule_args
        .strip_prefix('!')
        .map_or((false, rule_args), |pattern| (true, pattern));
    let hit = subjects
        .iter()
        .any(|subject| matches_positive_pattern(positive_pattern, subject));
    if negated { !hit } else { hit }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matching_handles_dot_slash_globstar_classes_and_case() {
        assert!(glob_match("./src/lib.rs", "src/**", false));
        assert!(glob_match("src/deep/lib.rs", "src/**", false));
        assert!(glob_match("file7.rs", "file[0-9].rs", false));
        assert!(glob_match("README", "readme", true));
        assert!(glob_match("main.rs", "{main,lib}.rs", false));
        assert!(!glob_match("src/deep/lib.rs", "src/*", false));
        assert!(!glob_match(".env", "*", false));
        assert!(!glob_match("src/.env", "**/*", false));
        assert!(glob_match("src/.env", "**/.env", false));
    }

    #[test]
    fn path_matching_canonicalizes_relative_home_and_windows_variants() {
        let posix = PermissionPathMatchOptions {
            cwd: Some("/workspace"),
            path_class: Some(PathClass::Posix),
            home_dir: Some("/home/user"),
            case_insensitive_paths: None,
        };
        assert!(path_glob_match("/workspace/src/a.ts", "./src/**", posix));
        assert!(path_glob_match(
            "/workspace/Secrets.env",
            "/workspace/secrets.env",
            posix
        ));
        assert!(path_glob_match("/home/user/notes", "~/notes", posix));
        assert!(path_glob_match(
            r"C:\Repo\src\a.rs",
            "c:/repo/src/**",
            PermissionPathMatchOptions::default()
        ));
    }

    #[test]
    fn rule_subjects_escape_literals_and_apply_negation() {
        let subject = "src/[draft]*.md";
        let literal = escape_rule_subject_literal(subject);
        assert_eq!(
            literal_rule_pattern("Read", subject),
            format!("Read({literal})")
        );
        assert!(matches_glob_rule_subject(&literal, subject));
        assert!(matches_glob_rule_subject("git *", "git status"));
        assert!(!matches_glob_rule_subject("git *", "npm test"));
        assert!(matches_glob_rule_subject("!git *", "npm test"));
        assert!(!matches_path_rule_subject(
            "!./src/**",
            "/workspace/src/a.ts",
            PermissionPathMatchOptions {
                cwd: Some("/workspace"),
                path_class: Some(PathClass::Posix),
                ..PermissionPathMatchOptions::default()
            }
        ));
        assert!(matches_glob_rule_subject("", "anything"));
    }
}
