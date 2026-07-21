use super::types::ParsedSlashInput;

/// Original:
///   apps/kimi-code/src/tui/commands/parse.ts
///   parseSlashInput()
pub fn parse_slash_input(input: &str) -> Option<ParsedSlashInput> {
    let command = input.strip_prefix('/')?.trim();
    if command.is_empty() {
        return None;
    }
    let (name, args) = command
        .find(' ')
        .map(|space| (&command[..space], command[space + 1..].trim()))
        .unwrap_or((command, ""));
    if name.contains('/') && !name.contains(':') {
        return None;
    }
    Some(ParsedSlashInput {
        name: name.to_owned(),
        args: args.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_arguments_and_namespaced_plugin_commands() {
        assert_eq!(
            parse_slash_input("/goal   ship feature  "),
            Some(ParsedSlashInput {
                name: "goal".to_owned(),
                args: "ship feature".to_owned()
            })
        );
        assert_eq!(
            parse_slash_input("/plugin:frontend/component arg"),
            Some(ParsedSlashInput {
                name: "plugin:frontend/component".to_owned(),
                args: "arg".to_owned()
            })
        );
    }

    #[test]
    fn rejects_plain_text_empty_commands_and_file_paths() {
        for input in ["goal", "/", "/   ", "/usr/local/bin"] {
            assert_eq!(parse_slash_input(input), None);
        }
    }

    #[test]
    fn only_literal_spaces_split_the_command_name() {
        assert_eq!(
            parse_slash_input("/goal\tstatus"),
            Some(ParsedSlashInput {
                name: "goal\tstatus".to_owned(),
                args: String::new()
            })
        );
    }
}
