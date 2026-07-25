use serde_json::{Map, Value};
use std::collections::HashSet;
use std::error::Error;
use std::fmt;

const TYPE_COMPLETION_SKIP_KEYS: &[&str] = &[
    "$ref", "allOf", "anyOf", "else", "if", "not", "oneOf", "then",
];
const OBJECT_STRUCTURE_KEYS: &[&str] = &[
    "dependencies",
    "dependentSchemas",
    "patternProperties",
    "properties",
    "additionalProperties",
    "propertyNames",
    "unevaluatedProperties",
    "dependentRequired",
    "maxProperties",
    "minProperties",
    "required",
];
const ARRAY_STRUCTURE_KEYS: &[&str] = &[
    "additionalItems",
    "contains",
    "unevaluatedItems",
    "prefixItems",
    "items",
    "maxContains",
    "maxItems",
    "minContains",
    "minItems",
    "uniqueItems",
];
const STRING_STRUCTURE_KEYS: &[&str] = &[
    "contentSchema",
    "contentEncoding",
    "contentMediaType",
    "format",
    "maxLength",
    "minLength",
    "pattern",
];
const NUMERIC_STRUCTURE_KEYS: &[&str] = &[
    "exclusiveMaximum",
    "exclusiveMinimum",
    "maximum",
    "minimum",
    "multipleOf",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KimiSchemaError(&'static str);

impl fmt::Display for KimiSchemaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for KimiSchemaError {}

// Original: kimi-schema.ts, derefJsonSchema().
pub fn deref_json_schema(schema: &Map<String, Value>) -> Map<String, Value> {
    let root = Value::Object(schema.clone());
    let mut visited = HashSet::new();
    let mut result = resolve_node(&root, &root, &mut visited)
        .as_object()
        .cloned()
        .unwrap_or_default();
    if !has_unresolved_definition_ref(&Value::Object(result.clone()), "$defs") {
        result.remove("$defs");
    }
    if !has_unresolved_definition_ref(&Value::Object(result.clone()), "definitions") {
        result.remove("definitions");
    }
    result
}

// Original: kimi-schema.ts, normalizeKimiToolSchema().
pub fn normalize_kimi_tool_schema(
    schema: &Map<String, Value>,
) -> Result<Map<String, Value>, KimiSchemaError> {
    let mut normalized = deref_json_schema(schema);
    recurse_schema(&mut normalized)?;
    Ok(normalized)
}

fn resolve_node(node: &Value, root: &Value, visited: &mut HashSet<String>) -> Value {
    match node {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| resolve_node(item, root, visited))
                .collect(),
        ),
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                if is_local_json_pointer_ref(reference)
                    && !visited.contains(reference)
                    && let Some(value) = resolve_local_json_pointer(root, reference)
                {
                    visited.insert(reference.to_owned());
                    let resolved = resolve_node(value, root, visited);
                    visited.remove(reference);
                    if let Value::Object(mut resolved_object) = resolved {
                        for (key, value) in object {
                            if key != "$ref" {
                                resolved_object
                                    .insert(key.clone(), resolve_node(value, root, visited));
                            }
                        }
                        return Value::Object(resolved_object);
                    }
                    return resolved;
                }
                return node.clone();
            }
            Value::Object(
                object
                    .iter()
                    .map(|(key, value)| (key.clone(), resolve_node(value, root, visited)))
                    .collect(),
            )
        }
        _ => node.clone(),
    }
}

fn is_local_json_pointer_ref(reference: &str) -> bool {
    reference == "#" || reference.starts_with("#/")
}

fn resolve_local_json_pointer<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    if reference == "#" {
        return Some(root);
    }
    let mut current = root;
    for raw_part in reference[2..].split('/') {
        let part = raw_part.replace("~1", "/").replace("~0", "~");
        current = match current {
            Value::Object(object) => object.get(&part)?,
            Value::Array(array) => {
                if part != "0" && part.starts_with('0')
                    || !part.bytes().all(|byte| byte.is_ascii_digit())
                {
                    return None;
                }
                array.get(part.parse::<usize>().ok()?)?
            }
            _ => return None,
        };
    }
    Some(current)
}

fn has_unresolved_definition_ref(node: &Value, bucket_key: &str) -> bool {
    match node {
        Value::Array(items) => items
            .iter()
            .any(|item| has_unresolved_definition_ref(item, bucket_key)),
        Value::Object(object) => {
            if object
                .get("$ref")
                .and_then(Value::as_str)
                .is_some_and(|reference| reference.starts_with(&format!("#/{bucket_key}/")))
            {
                return true;
            }
            object.iter().any(|(key, value)| {
                key != bucket_key && has_unresolved_definition_ref(value, bucket_key)
            })
        }
        _ => false,
    }
}

fn recurse_schema(node: &mut Map<String, Value>) -> Result<(), KimiSchemaError> {
    for key in [
        "$defs",
        "definitions",
        "dependencies",
        "dependentSchemas",
        "patternProperties",
        "properties",
    ] {
        if let Some(Value::Object(children)) = node.get_mut(key) {
            for child in children.values_mut() {
                normalize_property_value(child)?;
            }
        }
    }
    for key in [
        "additionalItems",
        "additionalProperties",
        "contains",
        "contentSchema",
        "else",
        "if",
        "not",
        "propertyNames",
        "then",
        "unevaluatedItems",
        "unevaluatedProperties",
    ] {
        if let Some(child @ Value::Object(_)) = node.get_mut(key) {
            normalize_property_value(child)?;
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(Value::Array(children)) = node.get_mut(key) {
            for child in children {
                normalize_property_value(child)?;
            }
        }
    }
    if let Some(items) = node.get_mut("items") {
        match items {
            Value::Object(_) => normalize_property_value(items)?,
            Value::Array(children) => {
                for child in children {
                    normalize_property_value(child)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn normalize_property_value(value: &mut Value) -> Result<(), KimiSchemaError> {
    let Value::Object(node) = value else {
        return Ok(());
    };
    normalize_property(node)
}

fn normalize_property(node: &mut Map<String, Value>) -> Result<(), KimiSchemaError> {
    let skip_completion = has_any_key(node, TYPE_COMPLETION_SKIP_KEYS);
    if !node.contains_key("type") && !skip_completion {
        let inferred = if let Some(Value::Array(values)) = node
            .get("enum")
            .filter(|v| v.as_array().is_some_and(|values| !values.is_empty()))
        {
            infer_type_from_values(values)?
        } else if let Some(value) = node.get("const") {
            infer_type_from_values(std::slice::from_ref(value))?
        } else {
            infer_type_from_structure(node)
        };
        node.insert("type".to_owned(), Value::String(inferred.to_owned()));
    } else if !skip_completion && node.get("type").is_some_and(Value::is_string) {
        let inferred = match node.get("enum") {
            Some(Value::Array(values)) if !values.is_empty() => infer_type_from_values(values).ok(),
            _ => node
                .get("const")
                .and_then(|value| infer_type_from_values(std::slice::from_ref(value)).ok()),
        };
        if let Some(inferred) = inferred
            && node.get("type").and_then(Value::as_str) != Some(inferred)
        {
            node.insert("type".to_owned(), Value::String(inferred.to_owned()));
            remove_irrelevant_structure_keys(node, inferred);
        }
    }
    recurse_schema(node)
}

fn remove_irrelevant_structure_keys(node: &mut Map<String, Value>, new_type: &str) {
    if new_type != "object" {
        for key in OBJECT_STRUCTURE_KEYS {
            node.remove(*key);
        }
    }
    if new_type != "array" {
        for key in ARRAY_STRUCTURE_KEYS {
            node.remove(*key);
        }
    }
}

fn infer_type_from_structure(schema: &Map<String, Value>) -> &'static str {
    if has_any_key(schema, OBJECT_STRUCTURE_KEYS) {
        "object"
    } else if has_any_key(schema, ARRAY_STRUCTURE_KEYS) {
        "array"
    } else if has_any_key(schema, STRING_STRUCTURE_KEYS) {
        "string"
    } else if has_any_key(schema, NUMERIC_STRUCTURE_KEYS) {
        "number"
    } else {
        "string"
    }
}

fn infer_type_from_values(values: &[Value]) -> Result<&'static str, KimiSchemaError> {
    let mut types = HashSet::new();
    for value in values {
        types.insert(match value {
            Value::Null => "null",
            Value::Array(_) => "array",
            Value::String(_) => "string",
            Value::Number(number)
                if number.is_i64()
                    || number.is_u64()
                    || number.as_f64().is_some_and(|value| value.fract() == 0.0) =>
            {
                "integer"
            }
            Value::Number(_) => "number",
            Value::Bool(_) => "boolean",
            Value::Object(_) => "object",
        });
    }
    if types.contains("number") {
        types.remove("integer");
    }
    let ordered = [
        "string", "number", "integer", "boolean", "object", "array", "null",
    ]
    .into_iter()
    .filter(|item| types.contains(item))
    .collect::<Vec<_>>();
    match ordered.as_slice() {
        [only] => Ok(*only),
        [] => Err(KimiSchemaError(
            "Cannot infer JSON Schema type from an empty enum.",
        )),
        _ => Err(KimiSchemaError(
            "Mixed JSON Schema enum or const types are not supported by Kimi tool schemas.",
        )),
    }
}

fn has_any_key(object: &Map<String, Value>, keys: &[&str]) -> bool {
    keys.iter().any(|key| object.contains_key(*key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn dereferences_local_refs_and_removes_unused_definition_buckets() {
        let schema = object(json!({
            "$defs": {"path": {"type": "string"}},
            "properties": {"path": {"$ref": "#/$defs/path", "description": "p"}}
        }));
        assert_eq!(
            deref_json_schema(&schema),
            object(json!({"properties":{"path":{"type":"string","description":"p"}}}))
        );
    }

    #[test]
    fn keeps_cycles_and_the_bucket_that_makes_them_resolvable() {
        let schema = object(json!({
            "$defs": {"node": {"properties": {"next": {"$ref": "#/$defs/node"}}}},
            "properties": {"root": {"$ref": "#/$defs/node"}}
        }));
        let result = deref_json_schema(&schema);
        assert!(result.contains_key("$defs"));
        assert_eq!(
            result["properties"]["root"]["properties"]["next"]["$ref"],
            "#/$defs/node"
        );
    }

    #[test]
    fn normalizes_structure_enum_const_and_nested_schema_slots() {
        let result = normalize_kimi_tool_schema(&object(json!({
            "properties": {
                "name": {},
                "count": {"minimum": 0},
                "choice": {"type": "object", "enum": ["a", "b"], "properties": {"x": {}}},
                "tuple": {"prefixItems": [{"const": true}]}
            }
        })))
        .unwrap();
        assert_eq!(result["properties"]["name"]["type"], "string");
        assert_eq!(result["properties"]["count"]["type"], "number");
        assert_eq!(result["properties"]["choice"]["type"], "string");
        assert!(result["properties"]["choice"].get("properties").is_none());
        assert_eq!(result["properties"]["tuple"]["type"], "array");
        assert_eq!(
            result["properties"]["tuple"]["prefixItems"][0]["type"],
            "boolean"
        );
    }

    #[test]
    fn rejects_mixed_enum_types_but_tolerates_mismatch_on_explicit_type() {
        let mixed = object(json!({"properties":{"v":{"enum":["x", 1]}}}));
        assert_eq!(
            normalize_kimi_tool_schema(&mixed).unwrap_err().to_string(),
            "Mixed JSON Schema enum or const types are not supported by Kimi tool schemas."
        );
        let explicit = object(json!({
            "properties":{"v":{"type":"object","enum":["x",1],"properties":{"x":{}}}}
        }));
        assert!(normalize_kimi_tool_schema(&explicit).is_ok());
    }

    #[test]
    fn resolves_escaped_pointer_parts_and_valid_array_indices_only() {
        let schema = object(json!({
            "$defs": {"a/b~c": [{"type":"string"}]},
            "properties": {
                "ok": {"$ref":"#/$defs/a~1b~0c/0"},
                "bad": {"$ref":"#/$defs/a~1b~0c/00"}
            }
        }));
        let result = deref_json_schema(&schema);
        assert_eq!(result["properties"]["ok"]["type"], "string");
        assert_eq!(result["properties"]["bad"]["$ref"], "#/$defs/a~1b~0c/00");
    }
}
