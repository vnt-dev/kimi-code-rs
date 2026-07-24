#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionSuffixParse<'a> {
    Bare { id: &'a str },
    Action { id: &'a str, action: String },
    Invalid { reason: String },
}

// Original: routes/action-suffix.ts, parseActionSuffix().
pub fn parse_action_suffix<'a>(
    tail: &'a str,
    allowed_actions: &[&str],
    default_action: Option<&str>,
    resource_label: Option<&str>,
) -> ActionSuffixParse<'a> {
    let label = resource_label.unwrap_or("resource");
    let Some(index) = tail.rfind(':') else {
        return parse_bare(tail, default_action, label);
    };
    if index == 0 {
        if tail.is_empty() {
            return invalid_id(label);
        }
        // Preserve the source quirk: ":dismiss" reaches the idx <= 0 bare
        // branch, so it is accepted as a bare id when a default exists.
        return if default_action.is_some() {
            ActionSuffixParse::Bare { id: tail }
        } else {
            ActionSuffixParse::Invalid {
                reason: format!("unsupported action: {tail}"),
            }
        };
    }
    let id = &tail[..index];
    let suffix = &tail[index + 1..];
    if suffix.is_empty() {
        return if default_action.is_some() {
            ActionSuffixParse::Bare { id: tail }
        } else {
            ActionSuffixParse::Invalid {
                reason: format!("unsupported action: {tail}"),
            }
        };
    }
    if id.is_empty() {
        return invalid_id(label);
    }
    match allowed_actions
        .iter()
        .copied()
        .find(|action| *action == suffix)
    {
        Some(action) => ActionSuffixParse::Action {
            id,
            action: action.to_owned(),
        },
        None => ActionSuffixParse::Invalid {
            reason: format!("unsupported action: {tail}"),
        },
    }
}

fn parse_bare<'a>(
    tail: &'a str,
    default_action: Option<&str>,
    label: &str,
) -> ActionSuffixParse<'a> {
    if tail.is_empty() {
        invalid_id(label)
    } else if default_action.is_some() {
        ActionSuffixParse::Bare { id: tail }
    } else {
        ActionSuffixParse::Invalid {
            reason: format!("unsupported action: {tail}"),
        }
    }
}

fn invalid_id(label: &str) -> ActionSuffixParse<'static> {
    ActionSuffixParse::Invalid {
        reason: format!("invalid {label}_id in path"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_actions_and_internal_colons() {
        assert_eq!(
            parse_action_suffix("q123", &["dismiss"], Some("resolve"), Some("question")),
            ActionSuffixParse::Bare { id: "q123" }
        );
        assert_eq!(
            parse_action_suffix(
                "mcp:lark:search:dismiss",
                &["dismiss"],
                Some("resolve"),
                None
            ),
            ActionSuffixParse::Action {
                id: "mcp:lark:search",
                action: "dismiss".into()
            }
        );
    }

    #[test]
    fn preserves_validation_messages_and_trailing_colon_behavior() {
        assert_eq!(
            parse_action_suffix("q123:foo", &["dismiss"], Some("resolve"), None),
            ActionSuffixParse::Invalid {
                reason: "unsupported action: q123:foo".into()
            }
        );
        assert_eq!(
            parse_action_suffix("q123:", &["dismiss"], Some("resolve"), None),
            ActionSuffixParse::Bare { id: "q123:" }
        );
        assert_eq!(
            parse_action_suffix("", &["dismiss"], Some("resolve"), Some("question")),
            ActionSuffixParse::Invalid {
                reason: "invalid question_id in path".into()
            }
        );
    }
}
