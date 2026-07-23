use crate::kosong::contract::message::ContentPart;

// Original:
//   packages/agent-core-v2/src/agent/contextMemory/vacuousContent.ts
//   isVacuousContentPart()
//
// Rust adaptation:
//   ECMAScript's String.trim whitespace set is spelled out because Rust's
//   char::is_whitespace additionally treats U+0085 as whitespace.
pub fn is_vacuous_content_part(part: &ContentPart) -> bool {
    match part {
        ContentPart::Text { text } => is_ecmascript_whitespace_only(text),
        ContentPart::Think { think, encrypted } => {
            encrypted.is_none() && is_ecmascript_whitespace_only(think)
        }
        ContentPart::ImageUrl { .. }
        | ContentPart::AudioUrl { .. }
        | ContentPart::VideoUrl { .. } => false,
    }
}

fn is_ecmascript_whitespace_only(value: &str) -> bool {
    value.chars().all(|character| {
        matches!(
            character,
            '\u{0009}'
                | '\u{000A}'
                | '\u{000B}'
                | '\u{000C}'
                | '\u{000D}'
                | '\u{0020}'
                | '\u{00A0}'
                | '\u{1680}'
                | '\u{2000}'
                ..='\u{200A}'
                    | '\u{2028}'
                    | '\u{2029}'
                    | '\u{202F}'
                    | '\u{205F}'
                    | '\u{3000}'
                    | '\u{FEFF}'
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kosong::contract::message::MediaUrl;

    #[test]
    fn empty_and_ecmascript_whitespace_text_are_vacuous() {
        assert!(is_vacuous_content_part(&ContentPart::Text {
            text: String::new(),
        }));
        assert!(is_vacuous_content_part(&ContentPart::Text {
            text: " \t\n\u{00a0}\u{2028}\u{feff}".to_owned(),
        }));
    }

    #[test]
    fn preserves_javascript_trim_boundary_behavior() {
        assert!(!is_vacuous_content_part(&ContentPart::Text {
            text: "\u{0085}".to_owned(),
        }));
    }

    #[test]
    fn only_unsigned_empty_thinking_is_vacuous() {
        assert!(is_vacuous_content_part(&ContentPart::Think {
            think: " \n".to_owned(),
            encrypted: None,
        }));
        assert!(!is_vacuous_content_part(&ContentPart::Think {
            think: String::new(),
            encrypted: Some(String::new()),
        }));
        assert!(!is_vacuous_content_part(&ContentPart::Think {
            think: "reasoning".to_owned(),
            encrypted: None,
        }));
    }

    #[test]
    fn media_content_is_never_vacuous() {
        assert!(!is_vacuous_content_part(&ContentPart::ImageUrl {
            image_url: MediaUrl {
                url: String::new(),
                id: None,
            },
        }));
    }
}
