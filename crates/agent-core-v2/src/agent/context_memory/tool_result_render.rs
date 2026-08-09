use crate::kosong::contract::message::ContentPart;

const TOOL_ERROR_STATUS: &str = "<system>ERROR: Tool execution failed.</system>";
const TOOL_EMPTY_STATUS: &str = "<system>Tool output is empty.</system>";
const TOOL_EMPTY_ERROR_STATUS: &str =
    "<system>ERROR: Tool execution failed. Tool output is empty.</system>";
const TOOL_OUTPUT_EMPTY_TEXT: &str = "Tool output is empty.";

#[derive(Clone, Copy, Debug)]
pub enum RenderableToolOutput<'a> {
    Text(&'a str),
    Parts(&'a [ContentPart]),
}

#[derive(Clone, Copy, Debug)]
pub struct RenderableToolResult<'a> {
    pub output: RenderableToolOutput<'a>,
    pub note: Option<&'a str>,
    pub is_error: Option<bool>,
}

// Original:
//   packages/agent-core-v2/src/agent/contextMemory/toolResultRender.ts
//   renderToolResultForModel()
pub fn render_tool_result_for_model(result: RenderableToolResult<'_>) -> Vec<ContentPart> {
    let mut rendered = render_status(result);
    let Some(note) = result.note.filter(|note| !note.is_empty()) else {
        return rendered;
    };
    if let [ContentPart::Text { text }] = rendered.as_mut_slice() {
        text.push('\n');
        text.push_str(note);
        return rendered;
    }
    rendered.push(text_part(note));
    rendered
}

// Original: toolResultRender.ts, renderStatus().
fn render_status(result: RenderableToolResult<'_>) -> Vec<ContentPart> {
    let parts = match result.output {
        RenderableToolOutput::Text(output) => return render_single_text(output, result.is_error),
        RenderableToolOutput::Parts([ContentPart::Text { text }]) => {
            return render_single_text(text, result.is_error);
        }
        RenderableToolOutput::Parts(parts) => parts,
    };
    if is_empty_equivalent_content_array(parts) {
        return vec![text_part(if result.is_error == Some(true) {
            TOOL_EMPTY_ERROR_STATUS
        } else {
            TOOL_EMPTY_STATUS
        })];
    }
    let mut rendered = Vec::with_capacity(parts.len() + usize::from(result.is_error == Some(true)));
    if result.is_error == Some(true) {
        rendered.push(text_part(TOOL_ERROR_STATUS));
    }
    rendered.extend_from_slice(parts);
    rendered
}

fn render_single_text(output: &str, is_error: Option<bool>) -> Vec<ContentPart> {
    if is_error == Some(true) {
        if output.is_empty() {
            return vec![text_part(TOOL_EMPTY_ERROR_STATUS)];
        }
        return vec![text_part(&format!("{TOOL_ERROR_STATUS}\n{output}"))];
    }
    if is_empty_output_text(output) {
        vec![text_part(TOOL_EMPTY_STATUS)]
    } else {
        vec![text_part(output)]
    }
}

fn text_part(text: &str) -> ContentPart {
    ContentPart::Text {
        text: text.to_owned(),
    }
}

fn is_empty_output_text(output: &str) -> bool {
    let trimmed = output.trim();
    trimmed.is_empty() || trimmed == TOOL_OUTPUT_EMPTY_TEXT
}

fn is_empty_equivalent_content_array(output: &[ContentPart]) -> bool {
    output
        .iter()
        .all(|part| matches!(part, ContentPart::Text { text } if text.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kosong::contract::message::MediaUrl;

    fn texts(parts: &[ContentPart]) -> Vec<&str> {
        parts
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn renders_empty_and_failed_string_statuses() {
        let empty = render_tool_result_for_model(RenderableToolResult {
            output: RenderableToolOutput::Text(" \u{00a0}"),
            note: None,
            is_error: None,
        });
        assert_eq!(texts(&empty), [TOOL_EMPTY_STATUS]);

        let failed = render_tool_result_for_model(RenderableToolResult {
            output: RenderableToolOutput::Text("details"),
            note: None,
            is_error: Some(true),
        });
        assert_eq!(
            texts(&failed),
            ["<system>ERROR: Tool execution failed.</system>\ndetails"]
        );
    }

    #[test]
    fn uses_rust_trim_boundary() {
        let rendered = render_tool_result_for_model(RenderableToolResult {
            output: RenderableToolOutput::Text("\u{0085}"),
            note: None,
            is_error: None,
        });
        assert_eq!(texts(&rendered), [TOOL_EMPTY_STATUS]);
    }

    #[test]
    fn appends_notes_to_text_or_after_structured_parts() {
        let text = render_tool_result_for_model(RenderableToolResult {
            output: RenderableToolOutput::Text("output"),
            note: Some("note"),
            is_error: None,
        });
        assert_eq!(texts(&text), ["output\nnote"]);

        let media = [ContentPart::ImageUrl {
            image_url: MediaUrl {
                url: "image.png".into(),
                id: None,
            },
        }];
        let rendered = render_tool_result_for_model(RenderableToolResult {
            output: RenderableToolOutput::Parts(&media),
            note: Some("note"),
            is_error: Some(true),
        });
        assert_eq!(texts(&rendered), [TOOL_ERROR_STATUS, "note"]);
        assert!(matches!(rendered[1], ContentPart::ImageUrl { .. }));
    }

    #[test]
    fn empty_content_arrays_use_empty_status() {
        let empty = render_tool_result_for_model(RenderableToolResult {
            output: RenderableToolOutput::Parts(&[]),
            note: None,
            is_error: Some(true),
        });
        assert_eq!(texts(&empty), [TOOL_EMPTY_ERROR_STATUS]);
    }
}
