use std::{
    error::Error,
    fmt,
    path::{Component, Path, PathBuf},
};

use async_trait::async_trait;
use futures_util::future::join_all;

use crate::tui::config::validate_tui_config_toml;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoctorTarget {
    Config,
    Tui,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DoctorOptions {
    pub target: Option<DoctorTarget>,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationPathSegment {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValidationIssue {
    pub path: Vec<ValidationPathSegment>,
    pub message: String,
}

#[derive(Debug)]
pub struct DoctorRuntimeError {
    message: String,
    validation_issues: Option<Vec<ConfigValidationIssue>>,
}

impl DoctorRuntimeError {
    pub fn new(error: impl Error) -> Self {
        Self {
            message: error.to_string(),
            validation_issues: None,
        }
    }

    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            validation_issues: None,
        }
    }

    pub fn validation(issues: Vec<ConfigValidationIssue>) -> Self {
        Self {
            message: "configuration validation failed".to_owned(),
            validation_issues: Some(issues),
        }
    }
}

impl fmt::Display for DoctorRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for DoctorRuntimeError {}

#[async_trait]
pub trait DoctorRuntime: Send + Sync {
    fn current_dir(&self) -> PathBuf;

    async fn default_config_path(&self) -> Result<PathBuf, DoctorRuntimeError>;

    fn default_tui_config_path(&self) -> Result<PathBuf, DoctorRuntimeError>;

    fn file_exists(&self, path: &Path) -> bool;

    async fn read_text_file(&self, path: &Path) -> Result<String, DoctorRuntimeError>;

    async fn validate_config_toml(
        &self,
        text: &str,
        file_path: &Path,
    ) -> Result<(), DoctorRuntimeError>;

    fn write_stdout(&self, text: &str);

    fn write_stderr(&self, text: &str);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckLabel {
    Config,
    Tui,
}

impl CheckLabel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config.toml",
            Self::Tui => "tui.toml",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckSpec {
    label: CheckLabel,
    path: PathBuf,
    explicit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckStatus {
    Ok,
    Skip,
    Error,
}

impl CheckStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Skip => "SKIP",
            Self::Error => "ERROR",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckResult {
    label: CheckLabel,
    path: PathBuf,
    status: CheckStatus,
    message: Option<String>,
}

// Original:
//   apps/kimi-code/src/cli/sub/doctor.ts
//   handleDoctor()
pub async fn handle_doctor(
    runtime: &dyn DoctorRuntime,
    options: &DoctorOptions,
) -> Result<i32, DoctorRuntimeError> {
    let specs = build_check_specs(runtime, options, &runtime.current_dir()).await?;
    let results = join_all(specs.into_iter().map(|spec| check_toml_file(runtime, spec))).await;
    let issue_count = results
        .iter()
        .filter(|result| result.status == CheckStatus::Error)
        .count();
    let text = if issue_count == 0 {
        format_success(&results)
    } else {
        format_failure(&results, issue_count)
    };
    if issue_count == 0 {
        runtime.write_stdout(&text);
        Ok(0)
    } else {
        runtime.write_stderr(&text);
        Ok(1)
    }
}

async fn build_check_specs(
    runtime: &dyn DoctorRuntime,
    options: &DoctorOptions,
    cwd: &Path,
) -> Result<Vec<CheckSpec>, DoctorRuntimeError> {
    match options.target {
        Some(DoctorTarget::Config) => Ok(vec![CheckSpec {
            label: CheckLabel::Config,
            path: match options.path.as_deref() {
                Some(path) => resolve_input_path(path, cwd),
                None => runtime.default_config_path().await?,
            },
            explicit: options.path.is_some(),
        }]),
        Some(DoctorTarget::Tui) => Ok(vec![CheckSpec {
            label: CheckLabel::Tui,
            path: options.path.as_deref().map_or_else(
                || runtime.default_tui_config_path(),
                |path| Ok(resolve_input_path(path, cwd)),
            )?,
            explicit: options.path.is_some(),
        }]),
        None => Ok(vec![
            CheckSpec {
                label: CheckLabel::Config,
                path: runtime.default_config_path().await?,
                explicit: false,
            },
            CheckSpec {
                label: CheckLabel::Tui,
                path: runtime.default_tui_config_path()?,
                explicit: false,
            },
        ]),
    }
}

async fn check_toml_file(runtime: &dyn DoctorRuntime, spec: CheckSpec) -> CheckResult {
    if !runtime.file_exists(&spec.path) {
        return CheckResult {
            label: spec.label,
            path: spec.path,
            status: if spec.explicit {
                CheckStatus::Error
            } else {
                CheckStatus::Skip
            },
            message: Some(if spec.explicit {
                "File does not exist.".to_owned()
            } else {
                "File does not exist; built-in defaults will apply.".to_owned()
            }),
        };
    }

    let validation = match runtime.read_text_file(&spec.path).await {
        Ok(text) => match spec.label {
            CheckLabel::Config => runtime.validate_config_toml(&text, &spec.path).await,
            CheckLabel::Tui => validate_tui_config_toml(&text).map_err(|error| {
                if error.issues.is_empty() {
                    DoctorRuntimeError::message(error.to_string())
                } else {
                    DoctorRuntimeError::validation(
                        error
                            .issues
                            .into_iter()
                            .map(|issue| ConfigValidationIssue {
                                path: issue
                                    .path
                                    .into_iter()
                                    .map(ValidationPathSegment::Key)
                                    .collect(),
                                message: issue.message,
                            })
                            .collect(),
                    )
                }
            }),
        },
        Err(error) => Err(error),
    };
    match validation {
        Ok(()) => CheckResult {
            label: spec.label,
            path: spec.path,
            status: CheckStatus::Ok,
            message: None,
        },
        Err(error) => CheckResult {
            label: spec.label,
            message: Some(format_error_message(&error, &spec.path)),
            path: spec.path,
            status: CheckStatus::Error,
        },
    }
}

fn resolve_input_path(input: &Path, cwd: &Path) -> PathBuf {
    if input.is_absolute() {
        input.to_path_buf()
    } else {
        normalize_path(&cwd.join(input))
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn format_success(results: &[CheckResult]) -> String {
    let mut lines = vec!["Kimi doctor".to_owned(), String::new()];
    lines.extend(format_results(results));
    lines.extend([
        String::new(),
        "All checked config files are valid.".to_owned(),
        String::new(),
    ]);
    lines.join("\n")
}

fn format_failure(results: &[CheckResult], issue_count: usize) -> String {
    let noun = if issue_count == 1 { "issue" } else { "issues" };
    let mut lines = vec![
        format!("Kimi doctor found {issue_count} {noun}."),
        String::new(),
    ];
    lines.extend(format_results(results));
    lines.push(String::new());
    lines.join("\n")
}

fn format_results(results: &[CheckResult]) -> Vec<String> {
    let mut lines = Vec::new();
    for result in results {
        lines.push(format!(
            "{} {:<12} {}",
            result.status.as_str(),
            result.label.as_str(),
            result.path.display()
        ));
        if let Some(message) = &result.message {
            lines.extend(message.split('\n').map(|line| format!("  {line}")));
        }
    }
    lines
}

fn format_error_message(error: &DoctorRuntimeError, file_path: &Path) -> String {
    let Some(issues) = &error.validation_issues else {
        return error.to_string();
    };
    let mut lines = vec![
        format!("Invalid configuration in {}.", file_path.display()),
        "Validation issues:".to_owned(),
    ];
    lines.extend(
        issues
            .iter()
            .map(|issue| format!("  {}: {}", format_issue_path(&issue.path), issue.message)),
    );
    lines.join("\n")
}

fn format_issue_path(path: &[ValidationPathSegment]) -> String {
    if path.is_empty() {
        return "<root>".to_owned();
    }
    let mut output = String::new();
    for segment in path {
        match segment {
            ValidationPathSegment::Index(index) => output.push_str(&format!("[{index}]")),
            ValidationPathSegment::Key(key) if output.is_empty() => {
                output.push_str(&camel_to_snake(key));
            }
            ValidationPathSegment::Key(key) => {
                output.push('.');
                output.push_str(&camel_to_snake(key));
            }
        }
    }
    output
}

fn camel_to_snake(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_uppercase() {
            output.push('_');
            output.push(character.to_ascii_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::Mutex,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    struct RuntimeMock {
        directory: PathBuf,
        stdout: Mutex<String>,
        stderr: Mutex<String>,
        default_config_calls: Mutex<usize>,
    }

    impl RuntimeMock {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let directory = std::env::temp_dir().join(format!("kimi-doctor-{unique}"));
            fs::create_dir_all(&directory).expect("temp directory");
            Self {
                directory,
                stdout: Mutex::new(String::new()),
                stderr: Mutex::new(String::new()),
                default_config_calls: Mutex::new(0),
            }
        }

        fn cleanup(&self) {
            fs::remove_dir_all(&self.directory).expect("cleanup");
        }
    }

    #[async_trait]
    impl DoctorRuntime for RuntimeMock {
        fn current_dir(&self) -> PathBuf {
            self.directory.clone()
        }

        async fn default_config_path(&self) -> Result<PathBuf, DoctorRuntimeError> {
            *self.default_config_calls.lock().expect("calls") += 1;
            Ok(self.directory.join("config.toml"))
        }

        fn default_tui_config_path(&self) -> Result<PathBuf, DoctorRuntimeError> {
            Ok(self.directory.join("tui.toml"))
        }

        fn file_exists(&self, path: &Path) -> bool {
            path.exists()
        }

        async fn read_text_file(&self, path: &Path) -> Result<String, DoctorRuntimeError> {
            fs::read_to_string(path).map_err(DoctorRuntimeError::new)
        }

        async fn validate_config_toml(
            &self,
            text: &str,
            _: &Path,
        ) -> Result<(), DoctorRuntimeError> {
            if text.contains("max_context_size = 0") {
                Err(DoctorRuntimeError::validation(vec![
                    ConfigValidationIssue {
                        path: vec![
                            ValidationPathSegment::Key("models".to_owned()),
                            ValidationPathSegment::Key("kimi".to_owned()),
                            ValidationPathSegment::Key("maxContextSize".to_owned()),
                        ],
                        message: "Must be greater than zero".to_owned(),
                    },
                ]))
            } else {
                Ok(())
            }
        }

        fn write_stdout(&self, text: &str) {
            self.stdout.lock().expect("stdout").push_str(text);
        }

        fn write_stderr(&self, text: &str) {
            self.stderr.lock().expect("stderr").push_str(text);
        }
    }

    #[tokio::test]
    async fn missing_default_files_are_skipped_without_failure() {
        let runtime = RuntimeMock::new();
        let code = handle_doctor(&runtime, &DoctorOptions::default())
            .await
            .expect("doctor");
        assert_eq!(code, 0);
        let output = runtime.stdout.lock().expect("stdout");
        assert!(output.contains("SKIP config.toml"));
        assert!(output.contains("SKIP tui.toml"));
        assert!(output.contains("built-in defaults will apply"));
        assert!(runtime.stderr.lock().expect("stderr").is_empty());
        drop(output);
        runtime.cleanup();
    }

    #[tokio::test]
    async fn missing_explicit_relative_path_is_an_error() {
        let runtime = RuntimeMock::new();
        let code = handle_doctor(
            &runtime,
            &DoctorOptions {
                target: Some(DoctorTarget::Config),
                path: Some(PathBuf::from("./nested/../missing.toml")),
            },
        )
        .await
        .expect("doctor");
        assert_eq!(code, 1);
        let error = runtime.stderr.lock().expect("stderr");
        assert!(error.contains("Kimi doctor found 1 issue."));
        assert!(error.contains(&runtime.directory.join("missing.toml").display().to_string()));
        assert!(error.contains("File does not exist."));
        assert!(!error.contains("tui.toml"));
        assert_eq!(*runtime.default_config_calls.lock().expect("calls"), 0);
        drop(error);
        runtime.cleanup();
    }

    #[tokio::test]
    async fn explicit_valid_config_checks_only_that_file() {
        let runtime = RuntimeMock::new();
        let path = runtime.directory.join("candidate.toml");
        fs::write(&path, "[models.kimi]\nmax_context_size = 1\n").expect("config");
        let code = handle_doctor(
            &runtime,
            &DoctorOptions {
                target: Some(DoctorTarget::Config),
                path: Some(PathBuf::from("candidate.toml")),
            },
        )
        .await
        .expect("doctor");
        assert_eq!(code, 0);
        let output = runtime.stdout.lock().expect("stdout");
        assert!(output.contains("OK config.toml"));
        assert!(!output.contains("tui.toml"));
        assert!(output.contains("All checked config files are valid."));
        drop(output);
        runtime.cleanup();
    }

    #[tokio::test]
    async fn aggregates_config_and_tui_validation_issues_with_paths() {
        let runtime = RuntimeMock::new();
        fs::write(
            runtime.directory.join("config.toml"),
            "[models.kimi]\nmax_context_size = 0\n",
        )
        .expect("config");
        fs::write(
            runtime.directory.join("tui.toml"),
            "editor = 123\n[notifications]\nenabled = \"yes\"\n",
        )
        .expect("tui");
        let code = handle_doctor(&runtime, &DoctorOptions::default())
            .await
            .expect("doctor");
        assert_eq!(code, 1);
        let error = runtime.stderr.lock().expect("stderr");
        assert!(error.contains("Kimi doctor found 2 issues."));
        assert!(error.contains("models.kimi.max_context_size:"));
        assert!(error.contains("editor:"));
        assert!(error.contains("notifications.enabled:"));
        assert!(runtime.stdout.lock().expect("stdout").is_empty());
        drop(error);
        runtime.cleanup();
    }

    #[test]
    fn issue_paths_support_root_indices_and_camel_case() {
        assert_eq!(format_issue_path(&[]), "<root>");
        assert_eq!(
            format_issue_path(&[
                ValidationPathSegment::Key("modelAliases".to_owned()),
                ValidationPathSegment::Index(2),
                ValidationPathSegment::Key("maxContextSize".to_owned()),
            ]),
            "model_aliases[2].max_context_size"
        );
    }
}
