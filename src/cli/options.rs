use std::{collections::HashMap, error::Error, fmt};

use super::experimental_v2::is_kimi_v2_enabled;

pub const OUTPUT_FORMAT_ENV: &str = "KIMI_MODEL_OUTPUT_FORMAT";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMode {
    Shell,
    Print,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptOutputFormat {
    Text,
    StreamJson,
}

impl PromptOutputFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::StreamJson => "stream-json",
        }
    }
}

impl TryFrom<&str> for PromptOutputFormat {
    type Error = OptionConflictError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "text" => Ok(Self::Text),
            "stream-json" => Ok(Self::StreamJson),
            value => Err(OptionConflictError::new(format!(
                "Invalid {OUTPUT_FORMAT_ENV} value \"{value}\". Expected one of: text, stream-json."
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CliOptions {
    pub session: Option<String>,
    pub continue_previous: bool,
    pub yolo: bool,
    pub auto: bool,
    pub plan: bool,
    pub model: Option<String>,
    pub output_format: Option<PromptOutputFormat>,
    pub prompt: Option<String>,
    pub skills_dirs: Vec<String>,
    pub agent: Option<String>,
    pub agent_files: Vec<String>,
    pub add_dirs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedOptions {
    pub options: CliOptions,
    pub ui_mode: UiMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionConflictError {
    message: String,
}

impl OptionConflictError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for OptionConflictError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for OptionConflictError {}

// Original:
//   apps/kimi-code/src/cli/options.ts
//   resolveOutputFormat()
pub fn resolve_output_format(
    options: &CliOptions,
    environment: &HashMap<String, String>,
) -> Result<PromptOutputFormat, OptionConflictError> {
    if let Some(output_format) = options.output_format {
        return Ok(output_format);
    }
    if options.prompt.is_none() {
        return Ok(PromptOutputFormat::Text);
    }
    let raw = environment
        .get(OUTPUT_FORMAT_ENV)
        .map_or("", String::as_str)
        .trim();
    if raw.is_empty() {
        return Ok(PromptOutputFormat::Text);
    }
    PromptOutputFormat::try_from(raw)
}

// Original:
//   apps/kimi-code/src/cli/options.ts
//   validateOptions()
pub fn validate_options(
    options: &CliOptions,
    environment: &HashMap<String, String>,
) -> Result<ValidatedOptions, OptionConflictError> {
    let prompt_mode = options.prompt.is_some();
    if options
        .prompt
        .as_deref()
        .is_some_and(|prompt| prompt.trim().is_empty())
    {
        return Err(OptionConflictError::new("Prompt cannot be empty."));
    }
    if options
        .model
        .as_deref()
        .is_some_and(|model| model.trim().is_empty())
    {
        return Err(OptionConflictError::new("Model cannot be empty."));
    }
    if !prompt_mode && options.output_format.is_some() {
        return Err(OptionConflictError::new(
            "Output format is only supported in prompt mode.",
        ));
    }
    if prompt_mode && options.yolo {
        return Err(OptionConflictError::new(
            "Cannot combine --prompt with --yolo.",
        ));
    }
    if prompt_mode && options.auto {
        return Err(OptionConflictError::new(
            "Cannot combine --prompt with --auto.",
        ));
    }
    if prompt_mode && options.plan {
        return Err(OptionConflictError::new(
            "Cannot combine --prompt with --plan.",
        ));
    }
    if options
        .agent
        .as_deref()
        .is_some_and(|agent| agent.trim().is_empty())
    {
        return Err(OptionConflictError::new("Agent cannot be empty."));
    }
    if options.agent_files.len() > 1 {
        return Err(OptionConflictError::new(
            "--agent-file may only be specified once.",
        ));
    }
    if options
        .agent_files
        .iter()
        .any(|file| file.trim().is_empty())
    {
        return Err(OptionConflictError::new("Agent file path cannot be empty."));
    }
    if options.agent.is_some() && !options.agent_files.is_empty() {
        return Err(OptionConflictError::new(
            "Cannot combine --agent with --agent-file.",
        ));
    }
    if (options.agent.is_some() || !options.agent_files.is_empty())
        && (!prompt_mode || !is_kimi_v2_enabled(environment))
    {
        return Err(OptionConflictError::new(
            "--agent/--agent-file are only available with the v2 engine (kimi -p with KIMI_CODE_EXPERIMENTAL_FLAG=1).",
        ));
    }
    if prompt_mode && options.session.as_deref() == Some("") {
        return Err(OptionConflictError::new(
            "Cannot use --session without an id in prompt mode.",
        ));
    }
    if options.continue_previous && options.session.is_some() {
        return Err(OptionConflictError::new(
            "Cannot combine --continue, --session.",
        ));
    }
    if options.yolo && options.auto {
        return Err(OptionConflictError::new(
            "Cannot combine --yolo with --auto.",
        ));
    }
    if prompt_mode {
        resolve_output_format(options, environment)?;
    }

    Ok(ValidatedOptions {
        options: options.clone(),
        ui_mode: if prompt_mode {
            UiMode::Print
        } else {
            UiMode::Shell
        },
    })
}

pub fn process_environment() -> HashMap<String, String> {
    std::env::vars().collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        CliOptions, OUTPUT_FORMAT_ENV, PromptOutputFormat, UiMode, resolve_output_format,
        validate_options,
    };
    use crate::cli::experimental_v2::KIMI_V2_ENV;

    fn prompt_options() -> CliOptions {
        CliOptions {
            prompt: Some("run this".to_owned()),
            ..CliOptions::default()
        }
    }

    fn assert_error(options: &CliOptions, expected: &str) {
        assert_eq!(
            validate_options(options, &HashMap::new())
                .expect_err("options should be rejected")
                .to_string(),
            expected
        );
    }

    #[test]
    fn defaults_to_shell_and_allows_session_modes() {
        let options = CliOptions::default();
        assert_eq!(
            validate_options(&options, &HashMap::new())
                .expect("defaults are valid")
                .ui_mode,
            UiMode::Shell
        );

        for options in [
            CliOptions {
                auto: true,
                continue_previous: true,
                ..CliOptions::default()
            },
            CliOptions {
                yolo: true,
                session: Some("ses_123".to_owned()),
                ..CliOptions::default()
            },
            CliOptions {
                plan: true,
                continue_previous: true,
                ..CliOptions::default()
            },
        ] {
            assert_eq!(
                validate_options(&options, &HashMap::new())
                    .expect("shell combination is valid")
                    .ui_mode,
                UiMode::Shell
            );
        }
    }

    #[test]
    fn validates_prompt_and_model_text() {
        assert_error(
            &CliOptions {
                prompt: Some("   ".to_owned()),
                ..CliOptions::default()
            },
            "Prompt cannot be empty.",
        );
        assert_error(
            &CliOptions {
                model: Some("   ".to_owned()),
                ..CliOptions::default()
            },
            "Model cannot be empty.",
        );
    }

    #[test]
    fn validates_prompt_mode_conflicts() {
        let mut options = prompt_options();
        options.yolo = true;
        assert_error(&options, "Cannot combine --prompt with --yolo.");

        let mut options = prompt_options();
        options.auto = true;
        assert_error(&options, "Cannot combine --prompt with --auto.");

        let mut options = prompt_options();
        options.plan = true;
        assert_error(&options, "Cannot combine --prompt with --plan.");

        let mut options = prompt_options();
        options.session = Some(String::new());
        assert_error(
            &options,
            "Cannot use --session without an id in prompt mode.",
        );
    }

    #[test]
    fn allows_prompt_resume_and_continue() {
        for options in [
            CliOptions {
                continue_previous: true,
                ..prompt_options()
            },
            CliOptions {
                session: Some("ses_123".to_owned()),
                ..prompt_options()
            },
        ] {
            assert_eq!(
                validate_options(&options, &HashMap::new())
                    .expect("prompt session choice is valid")
                    .ui_mode,
                UiMode::Print
            );
        }
    }

    #[test]
    fn validates_shell_and_permission_conflicts() {
        assert_error(
            &CliOptions {
                output_format: Some(PromptOutputFormat::StreamJson),
                ..CliOptions::default()
            },
            "Output format is only supported in prompt mode.",
        );
        assert_error(
            &CliOptions {
                continue_previous: true,
                session: Some("ses_123".to_owned()),
                ..CliOptions::default()
            },
            "Cannot combine --continue, --session.",
        );
        assert_error(
            &CliOptions {
                yolo: true,
                auto: true,
                ..CliOptions::default()
            },
            "Cannot combine --yolo with --auto.",
        );
    }

    #[test]
    fn resolves_output_format_precedence() {
        assert_eq!(
            resolve_output_format(&prompt_options(), &HashMap::new()).expect("default format"),
            PromptOutputFormat::Text
        );
        let environment =
            HashMap::from([(OUTPUT_FORMAT_ENV.to_owned(), "  stream-json  ".to_owned())]);
        assert_eq!(
            resolve_output_format(&prompt_options(), &environment).expect("environment format"),
            PromptOutputFormat::StreamJson
        );
        assert_eq!(
            resolve_output_format(
                &CliOptions {
                    output_format: Some(PromptOutputFormat::Text),
                    ..prompt_options()
                },
                &environment,
            )
            .expect("flag format"),
            PromptOutputFormat::Text
        );
        assert_eq!(
            resolve_output_format(&CliOptions::default(), &environment)
                .expect("shell ignores environment"),
            PromptOutputFormat::Text
        );
    }

    #[test]
    fn rejects_invalid_output_environment_only_in_prompt_mode() {
        let environment = HashMap::from([(OUTPUT_FORMAT_ENV.to_owned(), "json".to_owned())]);
        let error = resolve_output_format(&prompt_options(), &environment)
            .expect_err("invalid format should fail");
        assert_eq!(
            error.to_string(),
            "Invalid KIMI_MODEL_OUTPUT_FORMAT value \"json\". Expected one of: text, stream-json."
        );
        assert!(validate_options(&prompt_options(), &environment).is_err());
        assert!(validate_options(&CliOptions::default(), &environment).is_ok());
    }

    #[test]
    fn validates_agent_selectors_and_v2_gate() {
        assert_error(
            &CliOptions {
                agent: Some("   ".to_owned()),
                ..prompt_options()
            },
            "Agent cannot be empty.",
        );
        assert_error(
            &CliOptions {
                agent_files: vec!["   ".to_owned()],
                ..prompt_options()
            },
            "Agent file path cannot be empty.",
        );
        assert_error(
            &CliOptions {
                agent_files: vec!["a.md".to_owned(), "b.md".to_owned()],
                ..prompt_options()
            },
            "--agent-file may only be specified once.",
        );
        assert_error(
            &CliOptions {
                agent: Some("reviewer".to_owned()),
                agent_files: vec!["reviewer.md".to_owned()],
                ..prompt_options()
            },
            "Cannot combine --agent with --agent-file.",
        );
        assert_error(
            &CliOptions {
                agent: Some("reviewer".to_owned()),
                ..prompt_options()
            },
            "--agent/--agent-file are only available with the v2 engine (kimi -p with KIMI_CODE_EXPERIMENTAL_FLAG=1).",
        );

        let environment = HashMap::from([(KIMI_V2_ENV.to_owned(), "1".to_owned())]);
        let validated = validate_options(
            &CliOptions {
                agent: Some("reviewer".to_owned()),
                ..prompt_options()
            },
            &environment,
        )
        .expect("v2 prompt agent is valid");
        assert_eq!(validated.ui_mode, UiMode::Print);
    }
}
