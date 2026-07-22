use std::{collections::HashMap, io, path::PathBuf, process::Stdio};

use async_trait::async_trait;
use tokio::process::Command;
use uuid::Uuid;

use crate::utils::shell_quote::quote_shell_arg;

pub fn resolve_editor_command(configured: Option<&str>) -> Option<String> {
    let environment = std::env::vars().collect::<HashMap<_, _>>();
    resolve_editor_command_from(configured, &environment)
}

// Original:
//   apps/kimi-code/src/utils/process/external-editor.ts
//   resolveEditorCommand()
pub fn resolve_editor_command_from(
    configured: Option<&str>,
    environment: &HashMap<String, String>,
) -> Option<String> {
    [
        configured,
        environment.get("VISUAL").map(String::as_str),
        environment.get("EDITOR").map(String::as_str),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|candidate| !candidate.is_empty())
    .map(str::to_owned)
}

#[async_trait]
pub trait ExternalEditorRuntime: Send + Sync {
    async fn run_shell(&self, command: &str) -> io::Result<i32>;
}

pub struct SystemExternalEditorRuntime;

#[async_trait]
impl ExternalEditorRuntime for SystemExternalEditorRuntime {
    async fn run_shell(&self, command: &str) -> io::Result<i32> {
        let mut process = if cfg!(windows) {
            let mut process = Command::new("cmd.exe");
            process.args(["/D", "/S", "/C", command]);
            process
        } else {
            let mut process = Command::new("sh");
            process.args(["-c", command]);
            process
        };
        let status = process
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await?;
        Ok(status.code().unwrap_or(0))
    }
}

// Original:
//   apps/kimi-code/src/utils/process/external-editor.ts
//   editInExternalEditor()
pub async fn edit_in_external_editor(
    initial_text: &str,
    command: &str,
) -> io::Result<Option<String>> {
    edit_in_external_editor_with(&SystemExternalEditorRuntime, initial_text, command).await
}

pub async fn edit_in_external_editor_with(
    runtime: &dyn ExternalEditorRuntime,
    initial_text: &str,
    command: &str,
) -> io::Result<Option<String>> {
    let directory = create_temp_directory().await?;
    let file = directory.join("prompt.md");
    let result = async {
        tokio::fs::write(&file, initial_text).await?;
        let shell_command = format!("{command} {}", quote_shell_arg(&file.to_string_lossy()));
        if runtime.run_shell(&shell_command).await? != 0 {
            return Ok(None);
        }
        match tokio::fs::read_to_string(&file).await {
            Ok(content) => Ok(Some(content)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&directory).await;
    result
}

async fn create_temp_directory() -> io::Result<PathBuf> {
    let root = std::env::temp_dir();
    for _ in 0..16 {
        let candidate = root.join(format!("kimi-edit-{}", Uuid::new_v4()));
        match tokio::fs::create_dir(&candidate).await {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "failed to create a unique external-editor directory",
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    struct EditingRuntime {
        exit_code: i32,
        replacement: Option<String>,
        commands: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl ExternalEditorRuntime for EditingRuntime {
        async fn run_shell(&self, command: &str) -> io::Result<i32> {
            self.commands
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(command.to_owned());
            if let Some(replacement) = &self.replacement {
                let path = extract_last_quoted_argument(command).expect("quoted prompt path");
                tokio::fs::write(path, replacement).await?;
            }
            Ok(self.exit_code)
        }
    }

    fn extract_last_quoted_argument(command: &str) -> Option<String> {
        let quote = if cfg!(windows) { '"' } else { '\'' };
        let end = command.rfind(quote)?;
        let start = command[..end].rfind(quote)?;
        Some(command[start + 1..end].to_owned())
    }

    #[test]
    fn configured_then_visual_then_editor_priority_ignores_blank_values() {
        let mut environment = HashMap::from([
            ("VISUAL".to_owned(), "nvim".to_owned()),
            ("EDITOR".to_owned(), "vim".to_owned()),
        ]);
        assert_eq!(
            resolve_editor_command_from(Some(" code --wait "), &environment).as_deref(),
            Some("code --wait")
        );
        assert_eq!(
            resolve_editor_command_from(None, &environment).as_deref(),
            Some("nvim")
        );
        environment.insert("VISUAL".to_owned(), "  ".to_owned());
        assert_eq!(
            resolve_editor_command_from(None, &environment).as_deref(),
            Some("vim")
        );
    }

    #[tokio::test]
    async fn returns_edited_content_and_quotes_temp_path() {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let runtime = EditingRuntime {
            exit_code: 0,
            replacement: Some("edited text".to_owned()),
            commands: Arc::clone(&commands),
        };
        let result = edit_in_external_editor_with(&runtime, "seed", "code --wait")
            .await
            .expect("editor");
        assert_eq!(result.as_deref(), Some("edited text"));
        let commands = commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(commands[0].starts_with("code --wait "));
        assert!(commands[0].contains("prompt.md"));
        let file = extract_last_quoted_argument(&commands[0]).expect("path");
        assert!(!std::path::Path::new(&file).exists());
    }

    #[tokio::test]
    async fn nonzero_exit_returns_none_and_still_cleans_up() {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let runtime = EditingRuntime {
            exit_code: 1,
            replacement: None,
            commands: Arc::clone(&commands),
        };
        assert_eq!(
            edit_in_external_editor_with(&runtime, "seed", "false")
                .await
                .expect("editor"),
            None
        );
        let command = commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[0]
            .clone();
        let file = extract_last_quoted_argument(&command).expect("path");
        assert!(!std::path::Path::new(&file).exists());
    }
}
