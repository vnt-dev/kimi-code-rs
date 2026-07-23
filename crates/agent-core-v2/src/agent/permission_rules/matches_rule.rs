use crate::tool::rule_match::glob_match;

use super::types::PermissionRule;

// Original:
//   packages/agent-core-v2/src/agent/permissionRules/matchesRule.ts
//   ParsedPattern / ParsedPermissionPattern
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedPattern {
    pub tool_name: String,
    pub arg_pattern: Option<String>,
}

pub type ParsedPermissionPattern = ParsedPattern;

#[derive(Clone, Copy, Default)]
pub struct PermissionRuleMatchExecution<'a> {
    pub matches_rule: Option<&'a dyn Fn(&str) -> bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionRuleMatchStrategy {
    ToolNameOnly,
    MatchesRule,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PermissionRuleMatch<'a> {
    pub rule: &'a PermissionRule,
    pub strategy: PermissionRuleMatchStrategy,
    pub has_rule_args: bool,
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum ParsePermissionPatternError {
    #[error("permission pattern: empty string")]
    Empty,
    #[error("permission pattern: missing closing paren in \"{0}\"")]
    MissingClosingParen(String),
    #[error("permission pattern: empty tool name in \"{0}\"")]
    EmptyToolName(String),
}

// Original: matchesRule.ts, parsePattern() / parsePermissionPattern().
pub fn parse_pattern(pattern: &str) -> Result<ParsedPattern, ParsePermissionPatternError> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Err(ParsePermissionPatternError::Empty);
    }

    let Some(open_index) = trimmed.find('(') else {
        return Ok(ParsedPattern {
            tool_name: trimmed.to_owned(),
            arg_pattern: None,
        });
    };
    if !trimmed.ends_with(')') {
        return Err(ParsePermissionPatternError::MissingClosingParen(
            pattern.to_owned(),
        ));
    }

    let tool_name = &trimmed[..open_index];
    if tool_name.is_empty() {
        return Err(ParsePermissionPatternError::EmptyToolName(
            pattern.to_owned(),
        ));
    }
    let arg_pattern = &trimmed[open_index + 1..trimmed.len() - 1];
    Ok(ParsedPattern {
        tool_name: tool_name.to_owned(),
        arg_pattern: (!arg_pattern.is_empty()).then(|| arg_pattern.to_owned()),
    })
}

pub use parse_pattern as parse_permission_pattern;

// Original: matchesRule.ts, matchPermissionRule().
pub fn match_permission_rule<'a>(
    rule: &'a PermissionRule,
    tool_name: &str,
    execution: PermissionRuleMatchExecution<'_>,
) -> Option<PermissionRuleMatch<'a>> {
    let parsed = parse_pattern(&rule.pattern).ok()?;
    if parsed.tool_name != "*" && !glob_match(tool_name, &parsed.tool_name, false) {
        return None;
    }
    let Some(arg_pattern) = parsed.arg_pattern else {
        return Some(PermissionRuleMatch {
            rule,
            strategy: PermissionRuleMatchStrategy::ToolNameOnly,
            has_rule_args: false,
        });
    };
    execution
        .matches_rule
        .is_some_and(|matches_rule| matches_rule(&arg_pattern))
        .then_some(PermissionRuleMatch {
            rule,
            strategy: PermissionRuleMatchStrategy::MatchesRule,
            has_rule_args: true,
        })
}

#[cfg(test)]
mod tests {
    use crate::tool::rule_match::{
        PermissionPathMatchOptions, matches_glob_rule_subject, matches_path_rule_subject,
    };

    use super::*;
    use crate::agent::permission_rules::types::{PermissionRuleDecision, PermissionRuleScope};
    use crate::tool::path_access::PathClass;

    fn rule(pattern: &str) -> PermissionRule {
        PermissionRule {
            decision: PermissionRuleDecision::Allow,
            scope: PermissionRuleScope::User,
            pattern: pattern.into(),
            reason: None,
        }
    }

    #[test]
    fn parses_bare_trimmed_and_argument_patterns() {
        assert_eq!(
            parse_pattern("  read  ").unwrap(),
            ParsedPattern {
                tool_name: "read".into(),
                arg_pattern: None,
            }
        );
        assert_eq!(
            parse_pattern("bash(src/**)").unwrap(),
            ParsedPattern {
                tool_name: "bash".into(),
                arg_pattern: Some("src/**".into()),
            }
        );
        assert_eq!(parse_pattern("bash()").unwrap().arg_pattern, None);
    }

    #[test]
    fn reports_original_parse_error_conditions_and_messages() {
        assert_eq!(
            parse_pattern("").unwrap_err().to_string(),
            "permission pattern: empty string"
        );
        assert_eq!(
            parse_pattern(" bash(src ").unwrap_err().to_string(),
            "permission pattern: missing closing paren in \" bash(src \""
        );
        assert_eq!(
            parse_pattern("(src)").unwrap_err().to_string(),
            "permission pattern: empty tool name in \"(src)\""
        );
    }

    #[test]
    fn matches_tool_name_only_and_glob_patterns() {
        let bash = rule("bash");
        let hit =
            match_permission_rule(&bash, "bash", PermissionRuleMatchExecution::default()).unwrap();
        assert_eq!(hit.rule, &bash);
        assert_eq!(hit.strategy, PermissionRuleMatchStrategy::ToolNameOnly);
        assert!(!hit.has_rule_args);
        assert!(
            match_permission_rule(&bash, "read", PermissionRuleMatchExecution::default()).is_none()
        );
        assert!(
            match_permission_rule(
                &rule("mcp__*"),
                "mcp__search",
                PermissionRuleMatchExecution::default()
            )
            .is_some()
        );
    }

    #[test]
    fn delegates_argument_matching_and_requires_a_matcher() {
        let bash = rule("Bash(git *)");
        let command = "git status";
        let matches = |pattern: &str| matches_glob_rule_subject(pattern, command);
        let hit = match_permission_rule(
            &bash,
            "Bash",
            PermissionRuleMatchExecution {
                matches_rule: Some(&matches),
            },
        )
        .unwrap();
        assert_eq!(hit.strategy, PermissionRuleMatchStrategy::MatchesRule);
        assert!(hit.has_rule_args);
        assert!(
            match_permission_rule(&bash, "Bash", PermissionRuleMatchExecution::default()).is_none()
        );
    }

    #[test]
    fn execution_matchers_preserve_glob_negation_and_path_semantics() {
        let workspace = PermissionPathMatchOptions {
            cwd: Some("/workspace"),
            path_class: Some(PathClass::Posix),
            ..PermissionPathMatchOptions::default()
        };
        let outside =
            |pattern: &str| matches_path_rule_subject(pattern, "/workspace/README.md", workspace);
        let inside =
            |pattern: &str| matches_path_rule_subject(pattern, "/workspace/src/a.ts", workspace);
        let edit = rule("Edit(!./src/**)");
        assert!(
            match_permission_rule(
                &edit,
                "Edit",
                PermissionRuleMatchExecution {
                    matches_rule: Some(&outside),
                }
            )
            .is_some()
        );
        assert!(
            match_permission_rule(
                &edit,
                "Edit",
                PermissionRuleMatchExecution {
                    matches_rule: Some(&inside),
                }
            )
            .is_none()
        );
    }

    #[test]
    fn invalid_rule_patterns_never_match() {
        assert!(
            match_permission_rule(&rule("("), "Bad", PermissionRuleMatchExecution::default())
                .is_none()
        );
    }
}
