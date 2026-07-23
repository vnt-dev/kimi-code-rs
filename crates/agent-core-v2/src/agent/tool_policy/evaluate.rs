use crate::tool::{ToolSource, rule_match::glob_match, tool_contract::is_mcp_tool_name};

// Original:
//   packages/agent-core-v2/src/agent/toolPolicy/evaluate.ts
//   ToolActivationPolicy
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ToolActivationPolicy {
    pub tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GlobalToolsPolicy {
    pub enabled: Option<Vec<String>>,
    pub disabled: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug)]
pub struct ToolPolicyLayers<'a> {
    pub profile: &'a ToolActivationPolicy,
    pub global: Option<&'a GlobalToolsPolicy>,
    pub session_disabled_tools: Option<&'a [String]>,
}

// Original: evaluate.ts, isToolActive().
pub fn is_tool_active(policy: &ToolActivationPolicy, name: &str, source: ToolSource) -> bool {
    if let Some(tools) = &policy.tools {
        let allowed = if source == ToolSource::Mcp {
            tools
                .iter()
                .any(|pattern| is_mcp_tool_name(pattern) && glob_match(name, pattern, false))
        } else {
            tools.iter().any(|candidate| candidate == name)
        };
        if !allowed {
            return false;
        }
    }

    let Some(disallowed_tools) = &policy.disallowed_tools else {
        return true;
    };
    if source == ToolSource::Mcp {
        !disallowed_tools
            .iter()
            .any(|pattern| is_mcp_tool_name(pattern) && glob_match(name, pattern, false))
    } else {
        !disallowed_tools.iter().any(|candidate| candidate == name)
    }
}

// Original: evaluate.ts, isToolActiveComposed().
pub fn is_tool_active_composed(
    layers: ToolPolicyLayers<'_>,
    name: &str,
    source: ToolSource,
) -> bool {
    if !is_tool_active(layers.profile, name, source) {
        return false;
    }

    let global_policy = layers.global.map(|global| ToolActivationPolicy {
        tools: global
            .enabled
            .as_ref()
            .filter(|enabled| !enabled.is_empty())
            .cloned(),
        disallowed_tools: global.disabled.clone(),
    });
    if global_policy
        .as_ref()
        .is_some_and(|policy| !is_tool_active(policy, name, source))
    {
        return false;
    }

    let session_policy = ToolActivationPolicy {
        tools: None,
        disallowed_tools: layers.session_disabled_tools.map(<[String]>::to_vec),
    };
    is_tool_active(&session_policy, name, source)
}

// Original: evaluate.ts, resolveActiveToolNames().
pub fn resolve_active_tool_names(policy: &ToolActivationPolicy) -> Option<Vec<String>> {
    policy.tools.as_ref().map(|tools| {
        tools
            .iter()
            .filter(|name| {
                let source = if is_mcp_tool_name(name) {
                    ToolSource::Mcp
                } else {
                    ToolSource::Builtin
                };
                is_tool_active(policy, name, source)
            })
            .cloned()
            .collect()
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InactiveToolPatternKind {
    WildcardNotMcp,
    IncompleteMcpName,
    UnknownTool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InactiveToolPattern {
    pub pattern: String,
    pub kind: InactiveToolPatternKind,
}

// Original: evaluate.ts, literalToolNames().
pub fn literal_tool_names(patterns: &[String]) -> Vec<String> {
    patterns
        .iter()
        .filter(|pattern| !is_mcp_tool_name(pattern) && !has_glob_magic(pattern))
        .cloned()
        .collect()
}

// Original: evaluate.ts, findInactiveToolPatterns().
pub fn find_inactive_tool_patterns(
    patterns: &[String],
    is_known_tool_name: Option<&dyn Fn(&str) -> bool>,
) -> Vec<InactiveToolPattern> {
    let mut issues = Vec::new();
    for pattern in patterns {
        let kind = if is_mcp_tool_name(pattern) {
            if !has_glob_magic(pattern) && !pattern["mcp__".len()..].contains("__") {
                Some(InactiveToolPatternKind::IncompleteMcpName)
            } else {
                None
            }
        } else if has_glob_magic(pattern) {
            Some(InactiveToolPatternKind::WildcardNotMcp)
        } else if is_known_tool_name.is_some_and(|is_known| !is_known(pattern)) {
            Some(InactiveToolPatternKind::UnknownTool)
        } else {
            None
        };
        if let Some(kind) = kind {
            issues.push(InactiveToolPattern {
                pattern: pattern.clone(),
                kind,
            });
        }
    }
    issues
}

fn has_glob_magic(pattern: &str) -> bool {
    pattern
        .chars()
        .any(|character| matches!(character, '*' | '?' | '[' | ']' | '{' | '}'))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn builtin_policy_uses_exact_allow_and_deny_names() {
        let policy = ToolActivationPolicy {
            tools: Some(strings(&["Read", "Bash*"])),
            disallowed_tools: Some(strings(&["Read"])),
        };
        assert!(!is_tool_active(&policy, "Read", ToolSource::Builtin));
        assert!(!is_tool_active(&policy, "Bash", ToolSource::Builtin));
        assert!(!is_tool_active(&policy, "Write", ToolSource::Builtin));

        let empty = ToolActivationPolicy {
            tools: Some(Vec::new()),
            disallowed_tools: None,
        };
        assert!(!is_tool_active(&empty, "Read", ToolSource::Builtin));
    }

    #[test]
    fn mcp_policy_filters_non_mcp_patterns_and_uses_globs() {
        let policy = ToolActivationPolicy {
            tools: Some(strings(&["*", "mcp__github__*"])),
            disallowed_tools: Some(strings(&["mcp__github__delete_*", "Read"])),
        };
        assert!(is_tool_active(
            &policy,
            "mcp__github__create_pr",
            ToolSource::Mcp
        ));
        assert!(!is_tool_active(
            &policy,
            "mcp__github__delete_repo",
            ToolSource::Mcp
        ));
        assert!(!is_tool_active(
            &policy,
            "mcp__linear__create_issue",
            ToolSource::Mcp
        ));
    }

    #[test]
    fn composed_policy_intersects_layers_but_empty_global_enabled_is_unconstrained() {
        let profile = ToolActivationPolicy {
            tools: Some(strings(&["Read", "Bash"])),
            disallowed_tools: None,
        };
        let global = GlobalToolsPolicy {
            enabled: Some(Vec::new()),
            disabled: Some(strings(&["Bash"])),
        };
        let session_disabled = strings(&["Write"]);
        let layers = ToolPolicyLayers {
            profile: &profile,
            global: Some(&global),
            session_disabled_tools: Some(&session_disabled),
        };
        assert!(is_tool_active_composed(layers, "Read", ToolSource::Builtin));
        assert!(!is_tool_active_composed(
            layers,
            "Bash",
            ToolSource::Builtin
        ));
        assert!(!is_tool_active_composed(
            layers,
            "Write",
            ToolSource::Builtin
        ));
    }

    #[test]
    fn resolves_allowlist_entries_that_survive_the_same_policy() {
        let policy = ToolActivationPolicy {
            tools: Some(strings(&["Read", "Bash", "mcp__github__*"])),
            disallowed_tools: Some(strings(&["Bash", "mcp__github__*"])),
        };
        assert_eq!(resolve_active_tool_names(&policy), Some(strings(&["Read"])));
        assert_eq!(
            resolve_active_tool_names(&ToolActivationPolicy::default()),
            None
        );
    }

    #[test]
    fn finds_inactive_patterns_in_input_order() {
        let known = strings(&["Read", "Bash", "Skill"])
            .into_iter()
            .collect::<HashSet<_>>();
        let is_known = |name: &str| known.contains(name);
        assert_eq!(
            find_inactive_tool_patterns(
                &strings(&[
                    "Read",
                    "Bashh",
                    "read",
                    "*",
                    "Bash*",
                    "mcp__github",
                    "mcp__",
                    "mcp__github__create_issue",
                    "mcp__github__*",
                    "mcp__*",
                ]),
                Some(&is_known),
            ),
            vec![
                InactiveToolPattern {
                    pattern: "Bashh".into(),
                    kind: InactiveToolPatternKind::UnknownTool,
                },
                InactiveToolPattern {
                    pattern: "read".into(),
                    kind: InactiveToolPatternKind::UnknownTool,
                },
                InactiveToolPattern {
                    pattern: "*".into(),
                    kind: InactiveToolPatternKind::WildcardNotMcp,
                },
                InactiveToolPattern {
                    pattern: "Bash*".into(),
                    kind: InactiveToolPatternKind::WildcardNotMcp,
                },
                InactiveToolPattern {
                    pattern: "mcp__github".into(),
                    kind: InactiveToolPatternKind::IncompleteMcpName,
                },
                InactiveToolPattern {
                    pattern: "mcp__".into(),
                    kind: InactiveToolPatternKind::IncompleteMcpName,
                },
            ]
        );
    }

    #[test]
    fn literal_names_exclude_mcp_and_glob_entries() {
        assert_eq!(
            literal_tool_names(&strings(&[
                "Read",
                "mcp__*",
                "Bash*",
                "mcp__github__create_issue"
            ])),
            strings(&["Read"])
        );
        assert!(find_inactive_tool_patterns(&strings(&["AnythingGoes"]), None).is_empty());
    }
}
