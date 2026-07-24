//! Normalized text view and mechanical replacements.
//!
//! Original: `packages/agent-core-v2/src/app/edit/textModel.ts`.

use crate::_base::text::line_endings::{
    LineEndingStyle, materialize_model_text, to_model_text_view,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextModel {
    pub line_ending_style: LineEndingStyle,
    pub text: String,
}
impl TextModel {
    pub fn new(raw: &str) -> Self {
        let view = to_model_text_view(raw);
        Self {
            text: view.text,
            line_ending_style: view.line_ending_style,
        }
    }
    pub fn count_occurrences(&self, needle: &str) -> usize {
        if needle.is_empty() {
            return 0;
        }
        let mut count = 0;
        let mut position = 0;
        while position < self.text.len() {
            let Some(index) = self.text[position..].find(needle) else {
                break;
            };
            count += 1;
            position += index + needle.len();
        }
        count
    }
    pub fn replace_once(&self, needle: &str, replacement: &str) -> String {
        self.text.find(needle).map_or_else(
            || self.text.clone(),
            |index| {
                format!(
                    "{}{}{}",
                    &self.text[..index],
                    replacement,
                    &self.text[index + needle.len()..]
                )
            },
        )
    }
    pub fn replace_all(&self, needle: &str, replacement: &str) -> (String, usize) {
        if needle.is_empty() {
            return (self.text.clone(), 0);
        }
        let count = self.count_occurrences(needle);
        (self.text.replace(needle, replacement), count)
    }
    pub fn materialize(&self, model_text: &str) -> String {
        materialize_model_text(model_text, self.line_ending_style)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_pure_crlf_for_matching_and_restores_it() {
        let model = TextModel::new("a\r\nb\r\n");
        assert_eq!(model.text, "a\nb\n");
        assert_eq!(model.count_occurrences("\n"), 2);
        assert_eq!(model.materialize(&model.replace_once("a\nb", "x")), "x\r\n");
    }
    #[test]
    fn overlapping_matches_follow_source_index_advance() {
        let model = TextModel::new("aaa");
        assert_eq!(model.count_occurrences("aa"), 1);
        assert_eq!(model.replace_all("aa", "x"), ("xa".into(), 1));
    }
}
