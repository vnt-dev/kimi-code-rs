use std::io::Write;

use quick_xml::Writer;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use serde_json::{Map, Value};

// Original: task/notificationXml.ts, renderNotificationXml(). Attribute
// values alone are escaped; model-visible text and child XML stay verbatim.
pub fn render_notification_xml(data: &Map<String, Value>) -> String {
    let id = string_attr(data.get("id"), "unknown");
    let category = string_attr(data.get("category"), "unknown");
    let notification_type = string_attr(data.get("type"), "unknown");
    let source_kind = string_attr(data.get("source_kind"), "unknown");
    let source_id = string_attr(data.get("source_id"), "unknown");
    let agent_id = optional_string_attr(data.get("agent_id"));
    let title = string_value(data.get("title"));
    let severity = string_value(data.get("severity"));
    let body = string_value(data.get("body"));
    let children_value = match data.get("children") {
        None | Some(Value::Null) => data.get("extraBlocks"),
        value => value,
    };

    let mut writer = Writer::new(Vec::new());
    let mut start = BytesStart::new("notification");
    start.push_attribute(("id", id.as_str()));
    start.push_attribute(("category", category.as_str()));
    start.push_attribute(("type", notification_type.as_str()));
    start.push_attribute(("source_kind", source_kind.as_str()));
    start.push_attribute(("source_id", source_id.as_str()));
    if let Some(agent_id) = agent_id {
        start.push_attribute(("agent_id", agent_id.as_str()));
    }
    writer
        .write_event(Event::Start(start))
        .expect("writing to Vec cannot fail");
    writer
        .get_mut()
        .write_all(b"\n")
        .expect("writing to Vec cannot fail");

    let mut blocks: Vec<String> = Vec::new();
    if !title.is_empty() {
        blocks.push(format!("Title: {title}"));
    }
    if !severity.is_empty() {
        blocks.push(format!("Severity: {severity}"));
    }
    if !body.is_empty() {
        blocks.push(body.into());
    }
    blocks.extend(child_blocks(children_value));
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            writer
                .get_mut()
                .write_all(b"\n")
                .expect("writing to Vec cannot fail");
        }
        writer
            .write_event(Event::Text(BytesText::from_escaped(block.as_str())))
            .expect("writing to Vec cannot fail");
    }
    if !blocks.is_empty() {
        writer
            .get_mut()
            .write_all(b"\n")
            .expect("writing to Vec cannot fail");
    }
    writer
        .write_event(Event::End(BytesEnd::new("notification")))
        .expect("writing to Vec cannot fail");
    String::from_utf8(writer.into_inner()).expect("render output is UTF-8")
}

fn string_attr(value: Option<&Value>, fallback: &str) -> String {
    optional_string_attr(value).unwrap_or_else(|| fallback.into())
}

fn optional_string_attr(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn string_value(value: Option<&Value>) -> &str {
    value.and_then(Value::as_str).unwrap_or("")
}

fn child_blocks(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(value)) if !value.is_empty() => vec![value.clone()],
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn renders_exact_tag_attributes_text_and_child_blocks() {
        let text = render_notification_xml(&object(serde_json::json!({
            "id": "n_\"1&2",
            "category": "task",
            "type": "task.done",
            "source_kind": "background_task",
            "source_id": "bg&1",
            "agent_id": "agent-\"0&",
            "title": "Task <finished>",
            "severity": "info",
            "body": "The task & completed.",
            "children": [
                "<output-file path=\"/tmp/a&amp;b\">result</output-file>",
                "",
                3
            ]
        })));
        assert_eq!(
            text,
            concat!(
                "<notification id=\"n_&quot;1&amp;2\" category=\"task\" type=\"task.done\" ",
                "source_kind=\"background_task\" source_id=\"bg&amp;1\" agent_id=\"agent-&quot;0&amp;\">\n",
                "Title: Task <finished>\n",
                "Severity: info\n",
                "The task & completed.\n",
                "<output-file path=\"/tmp/a&amp;b\">result</output-file>\n",
                "</notification>"
            )
        );
    }

    #[test]
    fn applies_fallbacks_omits_empty_fields_and_ignores_unrelated_values() {
        let text = render_notification_xml(&object(serde_json::json!({
            "id": "",
            "source_kind": "host",
            "agent_id": "",
            "title": 3,
            "tail_output": "must not appear"
        })));
        assert_eq!(
            text,
            concat!(
                "<notification id=\"unknown\" category=\"unknown\" type=\"unknown\" ",
                "source_kind=\"host\" source_id=\"unknown\">\n",
                "</notification>"
            )
        );
    }

    #[test]
    fn children_null_falls_back_but_invalid_non_null_children_do_not() {
        let fallback = render_notification_xml(&object(serde_json::json!({
            "children": null,
            "extraBlocks": "fallback"
        })));
        assert!(fallback.contains("\nfallback\n"));

        let blocked = render_notification_xml(&object(serde_json::json!({
            "children": false,
            "extraBlocks": "not selected"
        })));
        assert!(!blocked.contains("not selected"));
    }
}
