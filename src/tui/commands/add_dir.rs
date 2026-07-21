use std::{fmt::Display, future::Future};

use super::swarm::NO_ACTIVE_SESSION_MESSAGE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddDirChoice {
    Session,
    Remember,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddDirSessionSnapshot {
    pub id: String,
    pub additional_dirs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddDirPrompt {
    pub title: String,
    pub hint: &'static str,
    pub session_id: String,
    pub path: String,
    pub options: [(AddDirChoice, &'static str); 3],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddDirCommandPlan {
    Status(String),
    Error(&'static str),
    Prompt(AddDirPrompt),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddAdditionalDirResult {
    pub additional_dirs: Vec<String>,
    pub project_root: String,
    pub config_path: String,
    pub persisted: bool,
}

pub trait AddDirCommandHost {
    type Error: Display + Send;

    fn restore_editor(&mut self);
    fn current_session_id(&self) -> Option<&str>;
    fn add_additional_dir(
        &mut self,
        path: &str,
        persist: bool,
    ) -> impl Future<Output = Result<AddAdditionalDirResult, Self::Error>> + Send;
    fn update_additional_dirs(&mut self, additional_dirs: Vec<String>);
    fn refresh_slash_command_autocomplete(&mut self);
    fn show_status(&mut self, message: &str, success: bool);
    fn show_error(&mut self, message: &str);
}

/// Resolve listing, missing-session, and confirmation branches.
///
/// Original:
///   apps/kimi-code/src/tui/commands/add-dir.ts
///   handleAddDirCommand(), formatAdditionalDirsStatus()
pub fn plan_add_dir_command(
    session: Option<&AddDirSessionSnapshot>,
    args: &str,
) -> AddDirCommandPlan {
    let input = args.trim();
    if input.is_empty() || input.eq_ignore_ascii_case("list") {
        let additional_dirs = session
            .map(|session| session.additional_dirs.as_slice())
            .unwrap_or_default();
        return if additional_dirs.is_empty() {
            AddDirCommandPlan::Status("No additional directories configured.".to_owned())
        } else {
            AddDirCommandPlan::Status(format_additional_dirs_status(additional_dirs))
        };
    }
    let Some(session) = session else {
        return AddDirCommandPlan::Error(NO_ACTIVE_SESSION_MESSAGE);
    };
    AddDirCommandPlan::Prompt(AddDirPrompt {
        title: format!("Add directory to workspace: {input}"),
        hint: "↑↓ navigate · Enter confirm · Esc cancel",
        session_id: session.id.clone(),
        path: input.to_owned(),
        options: [
            (AddDirChoice::Session, "Yes, for this session"),
            (AddDirChoice::Remember, "Yes, and remember this directory"),
            (AddDirChoice::Cancel, "No"),
        ],
    })
}

pub fn format_additional_dirs_status(additional_dirs: &[String]) -> String {
    let mut lines = Vec::with_capacity(additional_dirs.len() + 1);
    lines.push("Additional directories:".to_owned());
    lines.extend(
        additional_dirs
            .iter()
            .map(|directory| format!("  {directory}")),
    );
    lines.join("\n")
}

/// Original:
///   apps/kimi-code/src/tui/commands/add-dir.ts
///   handleAddDirChoice()
pub async fn handle_add_dir_choice<H: AddDirCommandHost>(
    host: &mut H,
    prompt: &AddDirPrompt,
    choice: AddDirChoice,
) {
    host.restore_editor();
    if choice == AddDirChoice::Cancel {
        host.show_status(
            &format!("Did not add {} as a working directory.", prompt.path),
            false,
        );
        return;
    }
    if host.current_session_id() != Some(prompt.session_id.as_str()) {
        host.show_error(NO_ACTIVE_SESSION_MESSAGE);
        return;
    }

    let persist = choice == AddDirChoice::Remember;
    match host.add_additional_dir(&prompt.path, persist).await {
        Ok(result) => {
            host.update_additional_dirs(result.additional_dirs);
            host.refresh_slash_command_autocomplete();
            let message = if persist {
                format!(
                    "Added workspace directory:\n  {}\n  Saved to:\n  {}",
                    prompt.path, result.config_path
                )
            } else {
                format!(
                    "Added workspace directory:\n  {}\n  For this session only",
                    prompt.path
                )
            };
            host.show_status(&message, true);
        }
        Err(error) => host.show_error(&error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Host {
        session_id: Option<String>,
        result: Result<AddAdditionalDirResult, &'static str>,
        operations: Vec<String>,
    }

    impl AddDirCommandHost for Host {
        type Error = &'static str;

        fn restore_editor(&mut self) {
            self.operations.push("restore".to_owned());
        }

        fn current_session_id(&self) -> Option<&str> {
            self.session_id.as_deref()
        }

        async fn add_additional_dir(
            &mut self,
            path: &str,
            persist: bool,
        ) -> Result<AddAdditionalDirResult, Self::Error> {
            self.operations.push(format!("add:{path}:{persist}"));
            self.result.clone()
        }

        fn update_additional_dirs(&mut self, additional_dirs: Vec<String>) {
            self.operations
                .push(format!("dirs:{}", additional_dirs.join(",")));
        }

        fn refresh_slash_command_autocomplete(&mut self) {
            self.operations.push("refresh".to_owned());
        }

        fn show_status(&mut self, message: &str, success: bool) {
            self.operations.push(format!("status:{success}:{message}"));
        }

        fn show_error(&mut self, message: &str) {
            self.operations.push(format!("error:{message}"));
        }
    }

    fn snapshot() -> AddDirSessionSnapshot {
        AddDirSessionSnapshot {
            id: "session-1".to_owned(),
            additional_dirs: vec!["/repo/shared".to_owned(), "/repo/docs".to_owned()],
        }
    }

    fn prompt() -> AddDirPrompt {
        match plan_add_dir_command(Some(&snapshot()), " ../other ") {
            AddDirCommandPlan::Prompt(prompt) => prompt,
            plan => panic!("unexpected plan: {plan:?}"),
        }
    }

    fn host(result: Result<AddAdditionalDirResult, &'static str>) -> Host {
        Host {
            session_id: Some("session-1".to_owned()),
            result,
            operations: Vec::new(),
        }
    }

    #[test]
    fn lists_directories_for_empty_or_case_insensitive_list() {
        assert_eq!(
            plan_add_dir_command(Some(&snapshot()), "LIST"),
            AddDirCommandPlan::Status(
                "Additional directories:\n  /repo/shared\n  /repo/docs".to_owned()
            )
        );
        assert_eq!(
            plan_add_dir_command(None, ""),
            AddDirCommandPlan::Status("No additional directories configured.".to_owned())
        );
    }

    #[test]
    fn requires_session_only_when_adding_and_builds_choice_labels() {
        assert_eq!(
            plan_add_dir_command(None, "../other"),
            AddDirCommandPlan::Error(NO_ACTIVE_SESSION_MESSAGE)
        );
        let prompt = prompt();
        assert_eq!(prompt.title, "Add directory to workspace: ../other");
        assert_eq!(
            prompt.options.map(|(_, label)| label),
            [
                "Yes, for this session",
                "Yes, and remember this directory",
                "No"
            ]
        );
    }

    #[tokio::test]
    async fn adds_session_only_directory_and_refreshes_after_state_update() {
        let result = AddAdditionalDirResult {
            additional_dirs: vec!["../other".to_owned()],
            project_root: "/repo".to_owned(),
            config_path: "/repo/.kimi-code/local.toml".to_owned(),
            persisted: false,
        };
        let mut host = host(Ok(result));
        handle_add_dir_choice(&mut host, &prompt(), AddDirChoice::Session).await;
        assert_eq!(
            host.operations,
            [
                "restore",
                "add:../other:false",
                "dirs:../other",
                "refresh",
                "status:true:Added workspace directory:\n  ../other\n  For this session only",
            ]
        );
    }

    #[tokio::test]
    async fn remembered_choice_uses_config_path_in_success_message() {
        let result = AddAdditionalDirResult {
            additional_dirs: vec!["../other".to_owned()],
            project_root: "/repo".to_owned(),
            config_path: "/repo/.kimi-code/local.toml".to_owned(),
            persisted: true,
        };
        let mut host = host(Ok(result));
        handle_add_dir_choice(&mut host, &prompt(), AddDirChoice::Remember).await;
        assert!(host.operations.iter().any(|operation| operation ==
            "status:true:Added workspace directory:\n  ../other\n  Saved to:\n  /repo/.kimi-code/local.toml"));
    }

    #[tokio::test]
    async fn cancel_session_change_and_errors_short_circuit_mutation() {
        let mut cancelled = host(Err("unused"));
        handle_add_dir_choice(&mut cancelled, &prompt(), AddDirChoice::Cancel).await;
        assert_eq!(
            cancelled.operations,
            [
                "restore",
                "status:false:Did not add ../other as a working directory."
            ]
        );

        let mut changed = host(Err("unused"));
        changed.session_id = Some("session-2".to_owned());
        handle_add_dir_choice(&mut changed, &prompt(), AddDirChoice::Session).await;
        assert_eq!(
            changed.operations,
            [
                "restore".to_owned(),
                format!("error:{NO_ACTIVE_SESSION_MESSAGE}"),
            ]
        );

        let mut failed = host(Err("must exist and be a directory"));
        handle_add_dir_choice(&mut failed, &prompt(), AddDirChoice::Session).await;
        assert_eq!(
            failed.operations,
            [
                "restore",
                "add:../other:false",
                "error:must exist and be a directory"
            ]
        );
    }
}
