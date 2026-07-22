use crate::{sdk::types::PromptPart, tui::types::SteerInputItem};

/// Payload accepted by the session steering boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SteerPayload {
    Text(String),
    Parts(Vec<PromptPart>),
}

/// Flattens queued messages and the editor draft into the historical steering
/// payload while retaining extracted image and video parts.
///
/// Original:
///   apps/kimi-code/src/tui/kimi-tui.ts
///   combineSteerInput()
pub fn combine_steer_input(items: &[SteerInputItem]) -> SteerPayload {
    let has_media = items
        .iter()
        .any(|item| item.parts.as_ref().is_some_and(|parts| !parts.is_empty()));
    if !has_media {
        return SteerPayload::Text(
            items
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
    }

    let mut combined = Vec::new();
    for item in items {
        let item_parts = item.parts.as_deref().filter(|parts| !parts.is_empty());
        let starts_with_media = item_parts
            .and_then(|parts| parts.first())
            .is_some_and(|part| !is_text_part(part));
        let last_is_media = combined.last().is_some_and(|part| !is_text_part(part));
        if !(combined.is_empty() || last_is_media && starts_with_media) {
            append_steer_text(&mut combined, "\n\n");
        }

        if let Some(parts) = item_parts {
            for part in parts {
                match part {
                    PromptPart::Text { text } => append_steer_text(&mut combined, text),
                    part => combined.push(part.clone()),
                }
            }
        } else {
            append_steer_text(&mut combined, &item.text);
        }
    }
    SteerPayload::Parts(combined)
}

fn append_steer_text(parts: &mut Vec<PromptPart>, text: &str) {
    if let Some(PromptPart::Text { text: previous }) = parts.last_mut() {
        previous.push_str(text);
    } else {
        parts.push(PromptPart::Text {
            text: text.to_owned(),
        });
    }
}

fn is_text_part(part: &PromptPart) -> bool {
    matches!(part, PromptPart::Text { .. })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::types::MediaUrl;

    fn item(text: &str, parts: Option<Vec<PromptPart>>) -> SteerInputItem {
        SteerInputItem {
            text: text.to_owned(),
            parts,
            image_attachment_ids: None,
        }
    }

    fn image(id: &str) -> PromptPart {
        PromptPart::ImageUrl {
            image_url: MediaUrl {
                url: format!("data:image/png;base64,{id}"),
                id: Some(id.to_owned()),
            },
        }
    }

    fn video(id: &str) -> PromptPart {
        PromptPart::VideoUrl {
            video_url: MediaUrl {
                url: format!("file:///{id}.mp4"),
                id: Some(id.to_owned()),
            },
        }
    }

    #[test]
    fn joins_text_only_items_with_historical_separator() {
        assert_eq!(
            combine_steer_input(&[item("first", None), item("second", Some(Vec::new()))]),
            SteerPayload::Text("first\n\nsecond".to_owned())
        );
        assert_eq!(combine_steer_input(&[]), SteerPayload::Text(String::new()));
    }

    #[test]
    fn merges_adjacent_text_parts_and_separators() {
        let payload = combine_steer_input(&[
            item(
                "ignored display text",
                Some(vec![
                    PromptPart::Text {
                        text: "first".to_owned(),
                    },
                    PromptPart::Text {
                        text: " continued".to_owned(),
                    },
                ]),
            ),
            item("second", None),
        ]);
        assert_eq!(
            payload,
            SteerPayload::Parts(vec![PromptPart::Text {
                text: "first continued\n\nsecond".to_owned(),
            }])
        );
    }

    #[test]
    fn retains_separator_between_text_and_media() {
        assert_eq!(
            combine_steer_input(&[
                item("first", None),
                item("[Image #1]", Some(vec![image("1")]))
            ]),
            SteerPayload::Parts(vec![
                PromptPart::Text {
                    text: "first\n\n".to_owned(),
                },
                image("1"),
            ])
        );
    }

    #[test]
    fn drops_separator_only_between_touching_media_parts() {
        assert_eq!(
            combine_steer_input(&[
                item("[Image #1]", Some(vec![image("1")])),
                item("[Video #2]", Some(vec![video("2")])),
                item("after", None),
            ]),
            SteerPayload::Parts(vec![
                image("1"),
                video("2"),
                PromptPart::Text {
                    text: "\n\nafter".to_owned(),
                },
            ])
        );
    }
}
