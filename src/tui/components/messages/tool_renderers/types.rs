use crate::tui::{
    components::Component,
    types::{ToolCallBlockData, ToolResultBlockData},
};

pub const PREVIEW_LINES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RendererContext {
    pub expanded: bool,
}

pub type RenderedComponents = Vec<Box<dyn Component>>;
pub type ResultRenderer =
    fn(&ToolCallBlockData, &ToolResultBlockData, RendererContext) -> RenderedComponents;

// Original: tool-renderers/types.ts strArg()
pub fn str_arg<'a>(args: &'a serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> &'a str {
    keys.iter()
        .find_map(|key| {
            args.get(*key)
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_first_non_empty_string_argument() {
        let args = serde_json::json!({"first": "", "second": 2, "third": "value"});
        let args = args.as_object().expect("object");
        assert_eq!(str_arg(args, &["first", "second", "third"]), "value");
        assert_eq!(str_arg(args, &["missing"]), "");
    }
}
