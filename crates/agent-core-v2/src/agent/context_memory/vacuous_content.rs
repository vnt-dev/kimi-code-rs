use crate::kosong::contract::message::ContentPart;

// Original:
//   packages/agent-core-v2/src/agent/contextMemory/vacuousContent.ts
//   isVacuousContentPart()
//
pub fn is_vacuous_content_part(part: &ContentPart) -> bool {
    match part {
        ContentPart::Text { text } => text.trim().is_empty(),
        ContentPart::Think { think, encrypted } => encrypted.is_none() && think.trim().is_empty(),
        ContentPart::ImageUrl { .. }
        | ContentPart::AudioUrl { .. }
        | ContentPart::VideoUrl { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kosong::contract::message::MediaUrl;

    #[test]
    fn empty_and_rust_whitespace_text_are_vacuous() {
        assert!(is_vacuous_content_part(&ContentPart::Text {
            text: String::new(),
        }));
        assert!(is_vacuous_content_part(&ContentPart::Text {
            text: " \t\n\u{00a0}\u{2028}\u{0085}".to_owned(),
        }));
    }

    #[test]
    fn preserves_non_rust_trim_characters() {
        assert!(!is_vacuous_content_part(&ContentPart::Text {
            text: "\u{feff}".to_owned(),
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
