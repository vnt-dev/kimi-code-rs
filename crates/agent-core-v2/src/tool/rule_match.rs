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
    let (pattern, negated) = strip_pattern_negation(pattern);
    let matched = if contains_extglob(pattern) {
        ExtGlob::parse(pattern).is_some_and(|glob| glob.is_match(value, nocase))
    } else {
        GlobBuilder::new(pattern)
            .case_insensitive(nocase)
            .literal_separator(true)
            .backslash_escape(true)
            .build()
            .is_ok_and(|glob| {
                glob.compile_matcher().is_match(value) && dot_segments_are_explicit(value, pattern)
            })
    };
    matched != negated
}

fn strip_pattern_negation(mut pattern: &str) -> (&str, bool) {
    let mut negated = false;
    while pattern.starts_with('!') && !pattern.starts_with("!(") {
        negated = !negated;
        pattern = &pattern[1..];
    }
    (pattern, negated)
}

#[derive(Clone, Debug)]
enum GlobNode {
    Literal(char),
    AnyChar {
        allow_dot: bool,
    },
    Star {
        allow_dot: bool,
    },
    GlobStar {
        allow_dot: bool,
    },
    GlobStarSlash {
        allow_dot: bool,
    },
    Class(CharClass),
    Alternation(Vec<Vec<GlobNode>>),
    Repeat {
        alternatives: Vec<Vec<GlobNode>>,
        minimum: usize,
        optional: bool,
    },
    Negative(Vec<Vec<GlobNode>>),
}

#[derive(Clone, Debug)]
struct CharClass {
    negated: bool,
    ranges: Vec<(char, char)>,
}

#[derive(Clone, Debug)]
struct ExtGlob {
    nodes: Vec<GlobNode>,
}

impl ExtGlob {
    fn parse(pattern: &str) -> Option<Self> {
        let mut parser = GlobParser::new(pattern);
        let nodes = parser.parse_sequence(&[])?;
        (parser.position == parser.characters.len()).then_some(Self { nodes })
    }

    fn is_match(&self, value: &str, nocase: bool) -> bool {
        if value.is_empty() {
            return false;
        }
        let characters = value.chars().collect::<Vec<_>>();
        match_sequence(&self.nodes, &characters, 0, nocase).contains(&characters.len())
    }
}

struct GlobParser {
    characters: Vec<char>,
    position: usize,
    extglob_depth: usize,
}

impl GlobParser {
    fn new(pattern: &str) -> Self {
        Self {
            characters: pattern.chars().collect(),
            position: 0,
            extglob_depth: 0,
        }
    }

    fn parse_sequence(&mut self, stops: &[char]) -> Option<Vec<GlobNode>> {
        let mut nodes = Vec::new();
        while let Some(&character) = self.characters.get(self.position) {
            if stops.contains(&character) {
                break;
            }
            if matches!(character, '@' | '?' | '+' | '*' | '!')
                && self.characters.get(self.position + 1) == Some(&'(')
            {
                self.position += 2;
                self.extglob_depth += 1;
                let alternatives = self.parse_alternatives('|', ')');
                self.extglob_depth -= 1;
                let alternatives = alternatives?;
                nodes.push(match character {
                    '@' => GlobNode::Alternation(alternatives),
                    '?' => GlobNode::Repeat {
                        alternatives,
                        minimum: 0,
                        optional: true,
                    },
                    '+' => GlobNode::Repeat {
                        alternatives,
                        minimum: 1,
                        optional: false,
                    },
                    '*' => GlobNode::Repeat {
                        alternatives,
                        minimum: 0,
                        optional: false,
                    },
                    '!' => GlobNode::Negative(alternatives),
                    _ => return None,
                });
                continue;
            }
            match character {
                '\\' => {
                    self.position += 1;
                    nodes.push(GlobNode::Literal(*self.characters.get(self.position)?));
                    self.position += 1;
                }
                '?' => {
                    nodes.push(GlobNode::AnyChar {
                        allow_dot: self.extglob_depth > 0,
                    });
                    self.position += 1;
                }
                '*' => self.parse_star(&mut nodes),
                '[' => nodes.push(GlobNode::Class(self.parse_class()?)),
                '{' => {
                    let saved = self.position;
                    self.position += 1;
                    if let Some(alternatives) = self.parse_alternatives(',', '}')
                        && alternatives.len() > 1
                    {
                        nodes.push(GlobNode::Alternation(alternatives));
                    } else {
                        self.position = saved + 1;
                        nodes.push(GlobNode::Literal('{'));
                    }
                }
                _ => {
                    nodes.push(GlobNode::Literal(character));
                    self.position += 1;
                }
            }
        }
        Some(nodes)
    }

    fn parse_alternatives(&mut self, separator: char, end: char) -> Option<Vec<Vec<GlobNode>>> {
        let mut alternatives = Vec::new();
        loop {
            alternatives.push(self.parse_sequence(&[separator, end])?);
            match self.characters.get(self.position).copied() {
                Some(character) if character == separator => self.position += 1,
                Some(character) if character == end => {
                    self.position += 1;
                    return Some(alternatives);
                }
                _ => return None,
            }
        }
    }

    fn parse_star(&mut self, nodes: &mut Vec<GlobNode>) {
        let start = self.position;
        while self.characters.get(self.position) == Some(&'*') {
            self.position += 1;
        }
        let count = self.position - start;
        let at_segment_start = nodes
            .last()
            .is_none_or(|node| matches!(node, GlobNode::Literal('/')));
        let at_segment_end = self
            .characters
            .get(self.position)
            .is_none_or(|character| *character == '/');
        if count >= 2 && at_segment_start && at_segment_end {
            if self.characters.get(self.position) == Some(&'/') {
                self.position += 1;
                nodes.push(GlobNode::GlobStarSlash {
                    allow_dot: self.extglob_depth > 0,
                });
            } else {
                nodes.push(GlobNode::GlobStar {
                    allow_dot: self.extglob_depth > 0,
                });
            }
        } else {
            nodes.push(GlobNode::Star {
                allow_dot: self.extglob_depth > 0,
            });
        }
    }

    fn parse_class(&mut self) -> Option<CharClass> {
        self.position += 1;
        let negated = matches!(self.characters.get(self.position), Some('!' | '^'));
        if negated {
            self.position += 1;
        }
        let mut characters = Vec::new();
        let mut escaped = false;
        loop {
            let character = *self.characters.get(self.position)?;
            self.position += 1;
            if !escaped && character == ']' && !characters.is_empty() {
                break;
            }
            if !escaped && character == '\\' {
                escaped = true;
                continue;
            }
            characters.push(character);
            escaped = false;
        }
        let mut ranges = Vec::new();
        let mut index = 0;
        while index < characters.len() {
            if index + 2 < characters.len() && characters[index + 1] == '-' {
                ranges.push((characters[index], characters[index + 2]));
                index += 3;
            } else {
                ranges.push((characters[index], characters[index]));
                index += 1;
            }
        }
        Some(CharClass { negated, ranges })
    }
}

fn contains_extglob(pattern: &str) -> bool {
    let mut escaped = false;
    let characters = pattern.chars().collect::<Vec<_>>();
    for (index, character) in characters.iter().copied().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if matches!(character, '@' | '?' | '+' | '*' | '!')
            && characters.get(index + 1) == Some(&'(')
        {
            return true;
        }
    }
    false
}

fn match_sequence(nodes: &[GlobNode], value: &[char], start: usize, nocase: bool) -> Vec<usize> {
    let Some((node, remaining)) = nodes.split_first() else {
        return vec![start];
    };
    let ends = if let GlobNode::Negative(alternatives) = node {
        negative_ends(alternatives, remaining, value, start, nocase)
    } else {
        match_node(node, value, start, nocase)
    };
    let mut matches = Vec::new();
    for end in ends {
        matches.extend(match_sequence(remaining, value, end, nocase));
    }
    matches.sort_unstable();
    matches.dedup();
    matches
}

fn match_node(node: &GlobNode, value: &[char], start: usize, nocase: bool) -> Vec<usize> {
    match node {
        GlobNode::Literal(expected) => value
            .get(start)
            .filter(|actual| chars_equal(**actual, *expected, nocase))
            .map_or_else(Vec::new, |_| vec![start + 1]),
        GlobNode::AnyChar { allow_dot } => value
            .get(start)
            .filter(|character| {
                **character != '/'
                    && (*allow_dot || !(is_segment_start(value, start) && **character == '.'))
            })
            .map_or_else(Vec::new, |_| vec![start + 1]),
        GlobNode::Star { allow_dot } => star_ends(value, start, *allow_dot),
        GlobNode::GlobStar { allow_dot } => globstar_ends(value, start, *allow_dot),
        GlobNode::GlobStarSlash { allow_dot } => globstar_slash_ends(value, start, *allow_dot),
        GlobNode::Class(class) => value
            .get(start)
            .filter(|character| {
                **character != '/'
                    && (!is_segment_start(value, start)
                        || **character != '.'
                        || class.matches('.', nocase))
                    && class.matches(**character, nocase)
            })
            .map_or_else(Vec::new, |_| vec![start + 1]),
        GlobNode::Alternation(alternatives) => {
            match_alternatives(alternatives, value, start, nocase)
        }
        GlobNode::Repeat {
            alternatives,
            minimum,
            optional,
        } => repeat_ends(alternatives, value, start, nocase, *minimum, *optional),
        GlobNode::Negative(_) => Vec::new(),
    }
}

impl CharClass {
    fn matches(&self, character: char, nocase: bool) -> bool {
        let hit = self.ranges.iter().any(|(start, end)| {
            let character = fold_char(character, nocase);
            let start = fold_char(*start, nocase);
            let end = fold_char(*end, nocase);
            start <= character && character <= end
        });
        hit != self.negated
    }
}

fn match_alternatives(
    alternatives: &[Vec<GlobNode>],
    value: &[char],
    start: usize,
    nocase: bool,
) -> Vec<usize> {
    alternatives
        .iter()
        .flat_map(|alternative| match_sequence(alternative, value, start, nocase))
        .collect()
}

fn repeat_ends(
    alternatives: &[Vec<GlobNode>],
    value: &[char],
    start: usize,
    nocase: bool,
    minimum: usize,
    optional: bool,
) -> Vec<usize> {
    let first = match_alternatives(alternatives, value, start, nocase);
    if optional {
        let mut ends = first;
        ends.push(start);
        ends.sort_unstable();
        ends.dedup();
        return ends;
    }
    let mut reached = if minimum == 0 {
        vec![start]
    } else {
        Vec::new()
    };
    let mut frontier = first;
    reached.extend(frontier.iter().copied());
    let mut seen = reached.iter().copied().collect::<HashSet<_>>();
    seen.insert(start);
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for position in frontier {
            for end in match_alternatives(alternatives, value, position, nocase) {
                if end != position && seen.insert(end) {
                    reached.push(end);
                    next.push(end);
                }
            }
        }
        frontier = next;
    }
    reached.sort_unstable();
    reached.dedup();
    reached
}

fn negative_ends(
    alternatives: &[Vec<GlobNode>],
    remaining: &[GlobNode],
    value: &[char],
    start: usize,
    nocase: bool,
) -> Vec<usize> {
    let alternative_ends = match_alternatives(alternatives, value, start, nocase);
    let fixed_literal_with_suffix = !remaining.is_empty()
        && alternatives.iter().all(|alternative| {
            alternative
                .iter()
                .all(|node| matches!(node, GlobNode::Literal(_)))
        });
    let excluded = if fixed_literal_with_suffix {
        !alternative_ends.is_empty()
    } else {
        alternative_ends
            .into_iter()
            .any(|end| match_sequence(remaining, value, end, nocase).contains(&value.len()))
    };
    if excluded {
        Vec::new()
    } else {
        star_ends(value, start, true)
    }
}

fn star_ends(value: &[char], start: usize, allow_dot: bool) -> Vec<usize> {
    let mut ends = vec![start];
    if !allow_dot && value.get(start) == Some(&'.') && is_segment_start(value, start) {
        return ends;
    }
    for (offset, character) in value[start..].iter().enumerate() {
        if *character == '/' {
            break;
        }
        ends.push(start + offset + 1);
    }
    ends
}

fn globstar_ends(value: &[char], start: usize, allow_dot: bool) -> Vec<usize> {
    let mut ends = vec![start];
    for end in start + 1..=value.len() {
        if !allow_dot && value[end - 1] == '.' && is_segment_start(value, end - 1) {
            break;
        }
        ends.push(end);
    }
    ends
}

fn globstar_slash_ends(value: &[char], start: usize, allow_dot: bool) -> Vec<usize> {
    let mut ends = vec![start];
    let mut segment_start = start;
    for (index, character) in value.iter().enumerate().skip(start) {
        if !allow_dot && *character == '.' && index == segment_start {
            break;
        }
        if *character == '/' {
            ends.push(index + 1);
            segment_start = index + 1;
        }
    }
    ends
}

fn is_segment_start(value: &[char], position: usize) -> bool {
    position == 0 || value.get(position.wrapping_sub(1)) == Some(&'/')
}

fn chars_equal(left: char, right: char, nocase: bool) -> bool {
    left == right || (nocase && fold_char(left, true) == fold_char(right, true))
}

fn fold_char(character: char, nocase: bool) -> char {
    if nocase {
        character.to_lowercase().next().unwrap_or(character)
    } else {
        character
    }
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
    fn glob_matching_supports_positive_extglob_operators() {
        assert!(glob_match("foo", "@(foo|bar)", false));
        assert!(glob_match("bar", "@(foo|bar)", false));
        assert!(!glob_match("baz", "@(foo|bar)", false));
        assert!(glob_match("xy", "x?(foo|bar)y", false));
        assert!(glob_match("xfooy", "x?(foo|bar)y", false));
        assert!(!glob_match("xfoofooy", "x?(foo|bar)y", false));
        assert!(glob_match("foo", "+(foo|bar)", false));
        assert!(glob_match("foobarfoo", "+(foo|bar)", false));
        assert!(!glob_match("", "+(foo|bar)", false));
        assert!(!glob_match("", "*(foo|bar)", false));
        assert!(glob_match("foo", "*(foo|bar)", false));
        assert!(glob_match("barfoo", "*(foo|bar)", false));
    }

    #[test]
    fn glob_matching_scopes_negative_extglobs_to_the_following_suffix() {
        assert!(!glob_match("foo/bar.js", "foo/!(bar).js", false));
        assert!(!glob_match("foo/bart.js", "foo/!(bar).js", false));
        assert!(glob_match("foo/baz.js", "foo/!(bar).js", false));
        assert!(!glob_match("xfooy", "x!(foo|bar)y", false));
        assert!(glob_match("xbazy", "x!(foo|bar)y", false));
    }

    #[test]
    fn glob_matching_supports_nested_extglobs_and_other_glob_tokens() {
        assert!(glob_match("foo", "@(foo|b@(a|o)r)", false));
        assert!(glob_match("bar", "@(foo|b@(a|o)r)", false));
        assert!(glob_match("bor", "@(foo|b@(a|o)r)", false));
        assert!(glob_match("FILE7.rs", "@(file|lib)[0-9].rs", true));
        assert!(glob_match(
            "src/deep/main.rs",
            "@(src|test)/**/main.rs",
            false
        ));
        assert!(glob_match("src/main.rs", "@(src|test)/**/main.rs", false));
        assert!(!glob_match(
            "src/.hidden/main.rs",
            "@(src|test)/**/main.rs",
            false
        ));
        assert!(glob_match(".env", "@(.env|config)", false));
        assert!(glob_match(".env", "@(*|config)", false));
        assert!(!glob_match("foo.test.js", "!(*.test).js", false));
        assert!(glob_match("foo.testx.js", "!(*.test).js", false));
        assert!(glob_match("foo.js", "!(*.test).js", false));
    }

    #[test]
    fn glob_matching_preserves_picomatch_pattern_negation() {
        assert!(!glob_match("foo", "!foo", false));
        assert!(glob_match("bar", "!foo", false));
        assert!(glob_match("foo", "!!foo", false));
        assert!(!glob_match("bar", "!!foo", false));
        assert!(!glob_match("foo", "!(foo|bar)", false));
        assert!(glob_match("baz", "!(foo|bar)", false));
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
