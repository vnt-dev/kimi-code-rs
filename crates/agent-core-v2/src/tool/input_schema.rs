use serde_json::{Map, Value};

// Rust adaptation of toInputJsonSchema(): callers pass their generated input-view schema.
// Object nodes are closed recursively exactly as in the source helper.
pub fn to_input_json_schema(mut schema: Map<String, Value>) -> Map<String, Value> {
    close_object_nodes(&mut schema);
    schema
}

pub fn close_object_nodes(schema: &mut Map<String, Value>) {
    close_map(schema);
}

fn close_map(node: &mut Map<String, Value>) {
    if node.get("type").and_then(Value::as_str) == Some("object")
        && !node.contains_key("additionalProperties")
    {
        node.insert("additionalProperties".into(), Value::Bool(false));
    }
    for child in node.values_mut() {
        close_value(child);
    }
}

fn close_value(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                close_value(item);
            }
        }
        Value::Object(node) => {
            close_map(node);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closes_nested_objects_without_overwriting_explicit_policy() {
        let mut schema = serde_json::from_value::<Map<String, Value>>(serde_json::json!({
            "type": "object",
            "properties": {
                "nested": {
                    "type": "object",
                    "properties": {"name": {"type": "string"}}
                },
                "open": {"type": "object", "additionalProperties": true}
            }
        }))
        .unwrap();
        close_object_nodes(&mut schema);
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(
            schema["properties"]["nested"]["additionalProperties"],
            false
        );
        assert_eq!(schema["properties"]["open"]["additionalProperties"], true);
    }
}
