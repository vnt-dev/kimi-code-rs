use std::{collections::HashMap, sync::LazyLock};

use regex::{Captures, Regex};
use serde_json::Value;

static PROMPT_VARIABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").expect("prompt variable regex must compile")
});

// Original: packages/agent-core-v2/src/_base/utils/render-prompt.ts, renderPrompt().
pub fn render_prompt(template: &str, variables: &HashMap<String, Value>) -> String {
    PROMPT_VARIABLE
        .replace_all(template, |captures: &Captures<'_>| {
            let name = &captures[1];
            match variables.get(name) {
                Some(Value::String(value)) => value.clone(),
                Some(Value::Number(value)) => value.to_string(),
                _ => captures[0].to_owned(),
            }
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_only_known_string_and_number_variables_once() {
        let variables = HashMap::from([
            ("name".into(), Value::String("${count}".into())),
            ("count".into(), Value::from(3)),
            ("enabled".into(), Value::Bool(true)),
        ]);

        assert_eq!(
            render_prompt("$name ${name} ${count} ${enabled} ${unknown} $", &variables),
            "$name ${count} 3 ${enabled} ${unknown} $"
        );
    }
}
