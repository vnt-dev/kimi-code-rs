use regex::RegexBuilder;
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathToken {
    Key(String),
    Index(usize),
}

pub fn tokenize_path(path: &str) -> Vec<PathToken> {
    let mut tokens = Vec::new();
    for segment in path.split('.') {
        let mut remaining = segment;
        while !remaining.is_empty() {
            let Some(open) = remaining.find('[') else {
                tokens.push(PathToken::Key(remaining.to_owned()));
                break;
            };
            let Some(relative_close) = remaining[open + 1..].find(']') else {
                tokens.push(PathToken::Key(remaining.to_owned()));
                break;
            };
            let close = open + 1 + relative_close;
            if open > 0 {
                tokens.push(PathToken::Key(remaining[..open].to_owned()));
            }
            match remaining[open + 1..close].parse::<usize>() {
                Ok(index) => tokens.push(PathToken::Index(index)),
                Err(_) => {
                    tokens.push(PathToken::Key(remaining.to_owned()));
                    break;
                }
            }
            remaining = &remaining[close + 1..];
        }
    }
    tokens
}

// Original: packages/minidb/src/query.ts, getPath().
pub fn get_path<'a>(document: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = document;
    for token in tokenize_path(path) {
        current = match token {
            PathToken::Key(key) => current.as_object()?.get(&key)?,
            PathToken::Index(index) => current.as_array()?.get(index)?,
        };
    }
    Some(current)
}

// Original: packages/minidb/src/query.ts, setPath().
pub fn set_path(document: &mut Value, path: &str, value: Value) {
    let tokens = tokenize_path(path);
    if tokens.is_empty() {
        return;
    }
    set_tokens(document, &tokens, value);
}

fn set_tokens(current: &mut Value, tokens: &[PathToken], value: Value) {
    if tokens.len() == 1 {
        match &tokens[0] {
            PathToken::Key(key) => {
                if !current.is_object() {
                    *current = Value::Object(Map::new());
                }
                current
                    .as_object_mut()
                    .expect("object initialized")
                    .insert(key.clone(), value);
            }
            PathToken::Index(index) => {
                if !current.is_array() {
                    *current = Value::Array(Vec::new());
                }
                let array = current.as_array_mut().expect("array initialized");
                array.resize(index + 1, Value::Null);
                array[*index] = value;
            }
        }
        return;
    }

    let next_is_index = matches!(tokens[1], PathToken::Index(_));
    let child = match &tokens[0] {
        PathToken::Key(key) => {
            if !current.is_object() {
                *current = Value::Object(Map::new());
            }
            current
                .as_object_mut()
                .expect("object initialized")
                .entry(key.clone())
                .or_insert_with(|| {
                    if next_is_index {
                        Value::Array(Vec::new())
                    } else {
                        Value::Object(Map::new())
                    }
                })
        }
        PathToken::Index(index) => {
            if !current.is_array() {
                *current = Value::Array(Vec::new());
            }
            let array = current.as_array_mut().expect("array initialized");
            array.resize(index + 1, Value::Null);
            if !array[*index].is_object() && !array[*index].is_array() {
                array[*index] = if next_is_index {
                    Value::Array(Vec::new())
                } else {
                    Value::Object(Map::new())
                };
            }
            &mut array[*index]
        }
    };
    if !child.is_object() && !child.is_array() {
        *child = if next_is_index {
            Value::Array(Vec::new())
        } else {
            Value::Object(Map::new())
        };
    }
    set_tokens(child, &tokens[1..], value);
}

// Original: packages/minidb/src/query.ts, project().
pub fn project(document: &Value, paths: &[String]) -> Value {
    if paths.is_empty() {
        return document.clone();
    }
    let mut output = Value::Object(Map::new());
    for path in paths {
        if let Some(value) = get_path(document, path) {
            set_path(&mut output, path, value.clone());
        }
    }
    output
}

fn values_equal(left: Option<&Value>, right: &Value) -> bool {
    let Some(left) = left else {
        return false;
    };
    match (left.as_f64(), right.as_f64()) {
        (Some(left), Some(right)) => left == right,
        _ => left == right,
    }
}

fn compare_values(left: Option<&Value>, right: &Value) -> Option<std::cmp::Ordering> {
    let left = left?;
    if let (Some(left), Some(right)) = (left.as_f64(), right.as_f64()) {
        return left.partial_cmp(&right);
    }
    if let (Some(left), Some(right)) = (left.as_str(), right.as_str()) {
        return Some(left.cmp(right));
    }
    None
}

fn regex_matches(value: Option<&Value>, argument: &Value) -> bool {
    let Some(value) = value.and_then(Value::as_str) else {
        return false;
    };
    let (pattern, flags) = match argument {
        Value::Array(parts) => (
            parts.first().and_then(Value::as_str).unwrap_or_default(),
            parts.get(1).and_then(Value::as_str).unwrap_or_default(),
        ),
        Value::String(pattern) => (pattern.as_str(), ""),
        _ => return false,
    };
    let mut builder = RegexBuilder::new(pattern);
    builder
        .case_insensitive(flags.contains('i'))
        .multi_line(flags.contains('m'))
        .dot_matches_new_line(flags.contains('s'));
    builder.build().is_ok_and(|regex| regex.is_match(value))
}

fn matches_condition(value: Option<&Value>, condition: &Value) -> bool {
    let Some(operators) = condition.as_object() else {
        return values_equal(value, condition);
    };
    for (operator, argument) in operators {
        let matches = match operator.as_str() {
            "$eq" => values_equal(value, argument),
            "$ne" => !values_equal(value, argument),
            "$gt" => compare_values(value, argument).is_some_and(std::cmp::Ordering::is_gt),
            "$gte" => compare_values(value, argument).is_some_and(std::cmp::Ordering::is_ge),
            "$lt" => compare_values(value, argument).is_some_and(std::cmp::Ordering::is_lt),
            "$lte" => compare_values(value, argument).is_some_and(std::cmp::Ordering::is_le),
            "$in" => argument
                .as_array()
                .is_some_and(|items| items.iter().any(|item| values_equal(value, item))),
            "$nin" => argument
                .as_array()
                .is_some_and(|items| items.iter().all(|item| !values_equal(value, item))),
            "$regex" => regex_matches(value, argument),
            "$exists" => value.is_some() == argument.as_bool().unwrap_or(false),
            "$contains" => value
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().any(|item| values_equal(Some(item), argument))),
            "$type" => argument.as_str().is_some_and(|expected| match expected {
                "undefined" => value.is_none(),
                "object" => value
                    .is_some_and(|value| value.is_object() || value.is_array() || value.is_null()),
                "string" => value.is_some_and(Value::is_string),
                "number" => value.is_some_and(Value::is_number),
                "boolean" => value.is_some_and(Value::is_boolean),
                _ => false,
            }),
            _ => false,
        };
        if !matches {
            return false;
        }
    }
    true
}

// Original: packages/minidb/src/query.ts, match().
pub fn matches_filter(document: &Value, filter: Option<&Map<String, Value>>) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    if filter.is_empty() {
        return true;
    }
    for (key, condition) in filter {
        let matches = match key.as_str() {
            "$and" => condition.as_array().is_some_and(|filters| {
                filters
                    .iter()
                    .all(|filter| matches_filter(document, filter.as_object()))
            }),
            "$or" => condition.as_array().is_some_and(|filters| {
                filters
                    .iter()
                    .any(|filter| matches_filter(document, filter.as_object()))
            }),
            "$nor" => condition.as_array().is_some_and(|filters| {
                filters
                    .iter()
                    .all(|filter| !matches_filter(document, filter.as_object()))
            }),
            "$not" => !matches_filter(document, condition.as_object()),
            _ => matches_condition(get_path(document, key), condition),
        };
        if !matches {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn paths_projection_and_filters_match_source_contract() {
        let mut document = json!({"name":"Ann","age":30,"tags":["x","y"]});
        set_path(&mut document, "profile.items[1].name", json!("second"));
        assert_eq!(
            get_path(&document, "profile.items[1].name"),
            Some(&json!("second"))
        );
        assert_eq!(
            project(&document, &["name".into(), "profile.items[1].name".into()]),
            json!({
                "name":"Ann","profile":{"items":[null,{"name":"second"}]}
            })
        );

        for filter in [
            json!({"age":{"$gt":18}}),
            json!({"tags":{"$contains":"y"}}),
            json!({"$or":[{"age":{"$lt":18}},{"name":"Ann"}]}),
            json!({"name":{"$regex":"^A"}}),
        ] {
            assert!(matches_filter(&document, filter.as_object()));
        }
        assert!(!matches_filter(
            &document,
            json!({"age":{"$gte":31}}).as_object()
        ));
    }
}
