use regex::{Regex, RegexBuilder};

// Original: packages/agent-core-v2/src/_base/execEnv/globPattern.ts,
// globPatternToRegex().
pub fn glob_pattern_to_regex(pattern: &str, case_sensitive: bool) -> Result<Regex, regex::Error> {
    let characters = pattern.chars().collect::<Vec<_>>();
    let mut output = String::from("^");
    let mut index = 0;
    while index < characters.len() {
        match characters[index] {
            '*' => output.push_str("[^/]*"),
            '?' => output.push_str("[^/]"),
            '[' => {
                let end = characters[index + 1..]
                    .iter()
                    .position(|character| *character == ']')
                    .map(|offset| index + 1 + offset);
                if let Some(end) = end {
                    let mut class = characters[index + 1..end].iter().collect::<String>();
                    class = class.replace('\\', "\\\\");
                    if let Some(rest) = class.strip_prefix('!') {
                        class = format!("^{rest}");
                    } else if class.starts_with('^') {
                        class.insert(0, '\\');
                    }
                    output.push('[');
                    output.push_str(&class);
                    output.push(']');
                    index = end;
                } else {
                    output.push_str("\\[");
                }
            }
            '\\' => {
                if let Some(next) = characters.get(index + 1) {
                    output.push_str(&regex::escape(&next.to_string()));
                    index += 1;
                } else {
                    output.push_str("\\\\");
                }
            }
            character => output.push_str(&regex::escape(&character.to_string())),
        }
        index += 1;
    }
    output.push('$');
    RegexBuilder::new(&output)
        .case_insensitive(!case_sensitive)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_stars_questions_classes_negation_and_escapes() {
        let regex = glob_pattern_to_regex(r"file[!0-9]?\*.TXT", false).unwrap();
        assert!(regex.is_match("fileaX*.txt"));
        assert!(!regex.is_match("file2X*.txt"));
        assert!(!regex.is_match("dir/fileaX*.txt"));
        assert!(
            glob_pattern_to_regex("[abc", true)
                .unwrap()
                .is_match("[abc")
        );
    }
}
