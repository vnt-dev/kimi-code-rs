use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FuzzyMatch {
    pub matches: bool,
    pub score: f64,
}

// Original:
//   packages/pi-tui/src/fuzzy.ts
//   fuzzyMatch()
pub fn fuzzy_match(query: &str, text: &str) -> FuzzyMatch {
    let query_lower = query.to_lowercase();
    let text_lower = text.to_lowercase();
    let primary = match_normalized_query(&query_lower, &text_lower);
    if primary.matches {
        return primary;
    }

    let Some(swapped) = swapped_alpha_numeric_query(&query_lower) else {
        return primary;
    };
    let swapped_match = match_normalized_query(&swapped, &text_lower);
    if swapped_match.matches {
        FuzzyMatch {
            matches: true,
            score: swapped_match.score + 5.0,
        }
    } else {
        primary
    }
}

fn match_normalized_query(query: &str, text: &str) -> FuzzyMatch {
    let query: Vec<_> = query.chars().collect();
    let text: Vec<_> = text.chars().collect();
    if query.is_empty() {
        return FuzzyMatch {
            matches: true,
            score: 0.0,
        };
    }
    if query.len() > text.len() {
        return FuzzyMatch {
            matches: false,
            score: 0.0,
        };
    }

    let mut query_index = 0;
    let mut score = 0.0;
    let mut last_match_index: Option<usize> = None;
    let mut consecutive_matches = 0;
    for (index, character) in text.iter().enumerate() {
        if query_index >= query.len() {
            break;
        }
        if *character != query[query_index] {
            continue;
        }

        let word_boundary = index == 0 || is_word_boundary(text[index - 1]);
        if last_match_index == index.checked_sub(1) {
            consecutive_matches += 1;
            score -= f64::from(consecutive_matches * 5);
        } else {
            consecutive_matches = 0;
            if let Some(previous) = last_match_index {
                score += (index - previous - 1) as f64 * 2.0;
            }
        }
        if word_boundary {
            score -= 10.0;
        }
        score += index as f64 * 0.1;
        last_match_index = Some(index);
        query_index += 1;
    }

    if query_index < query.len() {
        return FuzzyMatch {
            matches: false,
            score: 0.0,
        };
    }
    if query == text {
        score -= 100.0;
    }
    FuzzyMatch {
        matches: true,
        score,
    }
}

fn is_word_boundary(character: char) -> bool {
    character.is_whitespace() || matches!(character, '-' | '_' | '.' | '/' | ':')
}

fn swapped_alpha_numeric_query(query: &str) -> Option<String> {
    let bytes = query.as_bytes();
    let first_digit = bytes.iter().position(u8::is_ascii_digit);
    if let Some(split) = first_digit
        && split > 0
        && bytes[..split].iter().all(u8::is_ascii_lowercase)
        && bytes[split..].iter().all(u8::is_ascii_digit)
    {
        return Some(format!("{}{}", &query[split..], &query[..split]));
    }

    let first_letter = bytes.iter().position(u8::is_ascii_lowercase);
    if let Some(split) = first_letter
        && split > 0
        && bytes[..split].iter().all(u8::is_ascii_digit)
        && bytes[split..].iter().all(u8::is_ascii_lowercase)
    {
        return Some(format!("{}{}", &query[split..], &query[..split]));
    }
    None
}

// Original:
//   packages/pi-tui/src/fuzzy.ts
//   fuzzyFilter()
pub fn fuzzy_filter<'a, T, F>(items: &'a [T], query: &str, get_text: F) -> Vec<&'a T>
where
    F: Fn(&T) -> String,
{
    if query.trim().is_empty() {
        return items.iter().collect();
    }
    let tokens: Vec<_> = query
        .trim()
        .split(|character: char| character.is_whitespace() || character == '/')
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.is_empty() {
        return items.iter().collect();
    }

    let mut results = Vec::new();
    for item in items {
        let text = get_text(item);
        let mut total_score = 0.0;
        let mut all_match = true;
        for token in &tokens {
            let result = fuzzy_match(token, &text);
            if result.matches {
                total_score += result.score;
            } else {
                all_match = false;
                break;
            }
        }
        if all_match {
            results.push((item, total_score));
        }
    }
    results.sort_by(|left, right| left.1.partial_cmp(&right.1).unwrap_or(Ordering::Equal));
    results.into_iter().map(|(item, _)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::{fuzzy_filter, fuzzy_match};

    #[test]
    fn matches_empty_exact_ordered_and_case_insensitive_queries() {
        assert_eq!(fuzzy_match("", "anything").score, 0.0);
        assert!(!fuzzy_match("longquery", "short").matches);
        assert!(fuzzy_match("test", "test").score < 0.0);
        assert!(fuzzy_match("abc", "aXbXc").matches);
        assert!(!fuzzy_match("abc", "cba").matches);
        assert!(fuzzy_match("ABC", "abc").matches);
        assert!(fuzzy_match("abc", "ABC").matches);
    }

    #[test]
    fn scores_consecutive_boundary_and_exact_matches_better() {
        assert!(fuzzy_match("foo", "foobar").score < fuzzy_match("foo", "f_o_o_bar").score);
        assert!(fuzzy_match("fb", "foo-bar").score < fuzzy_match("fb", "afbx").score);
        assert!(fuzzy_match("cl", "cl").score < fuzzy_match("cl", "clone").score);
    }

    #[test]
    fn matches_swapped_alpha_numeric_tokens() {
        assert!(fuzzy_match("codex52", "gpt-5.2-codex").matches);
        assert!(fuzzy_match("52codex", "gpt-codex-5.2").matches);
    }

    #[test]
    fn filters_and_sorts_by_match_quality() {
        let items = ["a_p_p", "app", "application", "banana"];
        let result = fuzzy_filter(&items, "app", |item| (*item).to_owned());
        assert_eq!(result[0], &"app");
        assert!(!result.contains(&&"banana"));
    }

    #[test]
    fn supports_slash_separated_tokens_in_reordered_text() {
        let items = [("gpt-5.5", "openai-codex")];
        let result = fuzzy_filter(&items, "openai-codex/gpt-5.5", |item| {
            format!("{} {}", item.0, item.1)
        });
        assert_eq!(result, [&items[0]]);
    }
}
