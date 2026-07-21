use super::types::AutocompleteItem;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgCompletionSpec {
    pub value: String,
    pub description: String,
}

/// Original:
///   apps/kimi-code/src/tui/commands/complete-args.ts
///   completeLeadingArg()
pub fn complete_leading_arg(
    specs: &[ArgCompletionSpec],
    argument_prefix: &str,
) -> Option<Vec<AutocompleteItem>> {
    if argument_prefix.contains(' ') {
        return None;
    }
    let lower = argument_prefix.to_lowercase();
    let items = specs
        .iter()
        .filter(|spec| spec.value.to_lowercase().starts_with(&lower))
        .map(|spec| AutocompleteItem {
            value: spec.value.clone(),
            label: spec.value.clone(),
            description: spec.description.clone(),
        })
        .collect::<Vec<_>>();
    if items.len() == 1 && items[0].value.to_lowercase() == lower {
        return None;
    }
    (!items.is_empty()).then_some(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specs() -> Vec<ArgCompletionSpec> {
        [
            ("status", "Show status"),
            ("start", "Start work"),
            ("pause", "Pause work"),
        ]
        .into_iter()
        .map(|(value, description)| ArgCompletionSpec {
            value: value.to_owned(),
            description: description.to_owned(),
        })
        .collect()
    }

    #[test]
    fn completes_the_first_token_case_insensitively() {
        let items = complete_leading_arg(&specs(), "ST").unwrap_or_default();
        assert_eq!(
            items
                .iter()
                .map(|item| item.value.as_str())
                .collect::<Vec<_>>(),
            ["status", "start"]
        );
        assert_eq!(items[0].label, "status");
        assert_eq!(items[0].description, "Show status");
    }

    #[test]
    fn suppresses_completed_unique_tokens_free_text_and_no_matches() {
        assert_eq!(complete_leading_arg(&specs(), "status"), None);
        assert_eq!(complete_leading_arg(&specs(), "pause objective"), None);
        assert_eq!(complete_leading_arg(&specs(), "missing"), None);
    }

    #[test]
    fn empty_prefix_returns_every_spec() {
        assert_eq!(
            complete_leading_arg(&specs(), "").map(|items| items.len()),
            Some(3)
        );
    }
}
