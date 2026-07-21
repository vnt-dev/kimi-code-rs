use std::{
    fmt::Display,
    future::Future,
    path::{Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, Utc};
use tokio::fs;
use url::Url;

use crate::{
    sdk::types::ContextMessage,
    tui::utils::export_markdown::{BuildExportMarkdownInput, build_export_markdown},
    utils::terminal_hyperlink::to_terminal_hyperlink,
};

use super::swarm::NO_ACTIVE_SESSION_MESSAGE;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkedSession {
    pub id: String,
    pub title: Option<String>,
}

pub trait SessionCommandHost {
    type Error: Display + Send;

    fn app_session_id(&self) -> &str;
    fn app_session_title(&self) -> Option<&str>;
    fn active_session_id(&self) -> Option<&str>;
    fn active_session_summary_title(&self) -> Option<&str>;

    fn rename_session(
        &mut self,
        session_id: &str,
        title: &str,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn fork_session(
        &mut self,
        session_id: &str,
        title: &str,
    ) -> impl Future<Output = Result<ForkedSession, Self::Error>> + Send;

    fn switch_to_session(
        &mut self,
        session: ForkedSession,
        status: &str,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn show_status(&mut self, message: &str);
    fn show_error(&mut self, message: &str);
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionExportContext {
    pub history: Vec<ContextMessage>,
    pub token_count: u64,
}

pub trait ExportMdCommandHost {
    type Error: Display + Send;

    fn active_session_id(&self) -> Option<&str>;
    fn work_dir(&self) -> &Path;
    fn get_session_context(
        &mut self,
    ) -> impl Future<Output = Result<SessionExportContext, Self::Error>> + Send;
    fn show_status(&mut self, message: &str);
    fn show_error(&mut self, message: &str);
    fn show_notice(&mut self, title: &str, body: &str);
}

/// Original:
///   apps/kimi-code/src/tui/commands/session.ts
///   handleTitleCommand()
pub async fn handle_title_command<H: SessionCommandHost>(host: &mut H, args: &str) {
    let title = args.trim();
    if title.is_empty() {
        let message = match host.app_session_title().filter(|title| !title.is_empty()) {
            Some(current) => format!("Session title: {current}"),
            None => format!("Session title: (not set) — id: {}", host.app_session_id()),
        };
        host.show_status(&message);
        return;
    }

    let Some(session_id) = host.active_session_id().map(str::to_owned) else {
        host.show_error(NO_ACTIVE_SESSION_MESSAGE);
        return;
    };
    let new_title = take_utf16_units(title, 200);
    match host.rename_session(&session_id, &new_title).await {
        Ok(()) => host.show_status(&format!("Session title set to: {new_title}")),
        Err(error) => host.show_error(&format!("Failed to set title: {error}")),
    }
}

/// Original:
///   apps/kimi-code/src/tui/commands/session.ts
///   handleForkCommand(), forkSourceTitle()
pub async fn handle_fork_command<H: SessionCommandHost>(host: &mut H) {
    let Some(session_id) = host.active_session_id().map(str::to_owned) else {
        host.show_error(NO_ACTIVE_SESSION_MESSAGE);
        return;
    };
    let source_title = fork_source_title(
        host.app_session_title(),
        host.active_session_summary_title(),
        &session_id,
    );
    let forked = match host
        .fork_session(&session_id, &format!("Fork: {source_title}"))
        .await
    {
        Ok(forked) => forked,
        Err(error) => {
            host.show_error(&format!("Failed to fork session: {error}"));
            return;
        }
    };
    let status = format!(
        "Session forked ({}). To return to the original session: kimi -r {session_id}",
        forked.id
    );
    if let Err(error) = host.switch_to_session(forked, &status).await {
        host.show_error(&format!("Failed to switch to forked session: {error}"));
    }
}

pub fn fork_source_title(
    current_title: Option<&str>,
    summary_title: Option<&str>,
    session_id: &str,
) -> String {
    if let Some(title) = current_title
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        return title.to_owned();
    }
    summary_title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or(session_id)
        .to_owned()
}

/// Original:
///   apps/kimi-code/src/tui/commands/session.ts
///   handleExportMdCommand()
pub async fn handle_export_md_command<H: ExportMdCommandHost>(host: &mut H, args: &str) {
    let now = DateTime::<Utc>::from(SystemTime::now());
    handle_export_md_command_at(host, args, now).await;
}

pub async fn handle_export_md_command_at<H: ExportMdCommandHost>(
    host: &mut H,
    args: &str,
    now: DateTime<Utc>,
) {
    let Some(session_id) = host.active_session_id().map(str::to_owned) else {
        host.show_error(NO_ACTIVE_SESSION_MESSAGE);
        return;
    };
    let work_dir = host.work_dir().to_path_buf();
    host.show_status("Exporting session as Markdown…");

    let context = match host.get_session_context().await {
        Ok(context) => context,
        Err(error) => {
            host.show_error(&format!("Failed to export session: {error}"));
            return;
        }
    };
    if context.history.is_empty() {
        host.show_error("No messages to export.");
        return;
    }

    let output_path = match resolve_export_path(args, &work_dir, &session_id, now) {
        Ok(path) => path,
        Err(error) => {
            host.show_error(&format!("Failed to export session: {error}"));
            return;
        }
    };
    let markdown = build_export_markdown(&BuildExportMarkdownInput {
        session_id: &session_id,
        work_dir: &work_dir.to_string_lossy(),
        history: &context.history,
        token_count: context.token_count,
        now,
    });
    if let Err(error) = write_export(&output_path, &markdown).await {
        host.show_error(&format!("Failed to export session: {error}"));
        return;
    }
    let Ok(url) = Url::from_file_path(&output_path) else {
        host.show_error("Failed to export session: could not create file URL");
        return;
    };
    let path_text = output_path.to_string_lossy();
    let linked = to_terminal_hyperlink(&path_text, url.as_str());
    host.show_notice(
        &format!("Exported {} messages", context.history.len()),
        &linked,
    );
}

fn resolve_export_path(
    args: &str,
    work_dir: &Path,
    session_id: &str,
    now: DateTime<Utc>,
) -> std::io::Result<PathBuf> {
    let trimmed = args.trim();
    if !trimmed.is_empty() {
        let path = PathBuf::from(trimmed);
        return if path.is_absolute() {
            Ok(path)
        } else {
            std::env::current_dir().map(|directory| directory.join(path))
        };
    }
    let short_id = take_utf16_units(session_id, 8);
    let timestamp = now.format("%Y%m%d-%H%M%S");
    Ok(work_dir.join(format!("kimi-export-{short_id}-{timestamp}.md")))
}

async fn write_export(path: &Path, markdown: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    fs::write(path, markdown).await
}

fn take_utf16_units(value: &str, maximum: usize) -> String {
    let mut units = 0;
    value
        .chars()
        .take_while(|character| {
            let next = units + character.len_utf16();
            if next > maximum {
                false
            } else {
                units = next;
                true
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use crate::sdk::types::{ContentPart, ContextMessageRole};

    use super::*;

    struct Host {
        app_id: String,
        app_title: Option<String>,
        active_id: Option<String>,
        summary_title: Option<String>,
        rename_result: Result<(), &'static str>,
        fork_result: Result<ForkedSession, &'static str>,
        switch_result: Result<(), &'static str>,
        operations: Vec<String>,
    }

    impl SessionCommandHost for Host {
        type Error = &'static str;

        fn app_session_id(&self) -> &str {
            &self.app_id
        }
        fn app_session_title(&self) -> Option<&str> {
            self.app_title.as_deref()
        }
        fn active_session_id(&self) -> Option<&str> {
            self.active_id.as_deref()
        }
        fn active_session_summary_title(&self) -> Option<&str> {
            self.summary_title.as_deref()
        }

        async fn rename_session(
            &mut self,
            session_id: &str,
            title: &str,
        ) -> Result<(), Self::Error> {
            self.operations.push(format!("rename:{session_id}:{title}"));
            self.rename_result
        }

        async fn fork_session(
            &mut self,
            session_id: &str,
            title: &str,
        ) -> Result<ForkedSession, Self::Error> {
            self.operations.push(format!("fork:{session_id}:{title}"));
            self.fork_result.clone()
        }

        async fn switch_to_session(
            &mut self,
            session: ForkedSession,
            status: &str,
        ) -> Result<(), Self::Error> {
            self.operations
                .push(format!("switch:{}:{status}", session.id));
            self.switch_result
        }

        fn show_status(&mut self, message: &str) {
            self.operations.push(format!("status:{message}"));
        }
        fn show_error(&mut self, message: &str) {
            self.operations.push(format!("error:{message}"));
        }
    }

    fn host() -> Host {
        Host {
            app_id: "session-1".to_owned(),
            app_title: None,
            active_id: Some("session-1".to_owned()),
            summary_title: None,
            rename_result: Ok(()),
            fork_result: Ok(ForkedSession {
                id: "fork-1".to_owned(),
                title: None,
            }),
            switch_result: Ok(()),
            operations: Vec::new(),
        }
    }

    #[tokio::test]
    async fn shows_or_sets_title_and_limits_it_by_utf16_units() {
        let mut value = host();
        handle_title_command(&mut value, "").await;
        assert_eq!(
            value.operations,
            ["status:Session title: (not set) — id: session-1"]
        );

        value.operations.clear();
        let long = format!("{}x", "a".repeat(199));
        handle_title_command(&mut value, &format!("  {long}\u{1f63a}tail  ")).await;
        assert_eq!(value.operations[0], format!("rename:session-1:{long}"));
        assert_eq!(
            value.operations[1],
            format!("status:Session title set to: {long}")
        );
    }

    #[tokio::test]
    async fn title_requires_session_and_reports_rename_failure() {
        let mut missing = host();
        missing.active_id = None;
        handle_title_command(&mut missing, "title").await;
        assert_eq!(
            missing.operations,
            [format!("error:{NO_ACTIVE_SESSION_MESSAGE}")]
        );

        let mut failed = host();
        failed.rename_result = Err("denied");
        handle_title_command(&mut failed, "title").await;
        assert_eq!(
            failed.operations,
            [
                "rename:session-1:title",
                "error:Failed to set title: denied"
            ]
        );
    }

    #[test]
    fn fork_title_prefers_app_then_summary_then_id() {
        assert_eq!(
            fork_source_title(Some(" Current "), Some("Summary"), "id"),
            "Current"
        );
        assert_eq!(
            fork_source_title(Some("  "), Some(" Summary "), "id"),
            "Summary"
        );
        assert_eq!(fork_source_title(None, Some(""), "id"), "id");
    }

    #[tokio::test]
    async fn forks_then_switches_with_return_instruction() {
        let mut value = host();
        value.app_title = Some("Source".to_owned());
        handle_fork_command(&mut value).await;
        assert_eq!(value.operations[0], "fork:session-1:Fork: Source");
        assert_eq!(
            value.operations[1],
            "switch:fork-1:Session forked (fork-1). To return to the original session: kimi -r session-1"
        );
    }

    #[tokio::test]
    async fn fork_and_switch_failures_have_distinct_messages() {
        let mut fork_failed = host();
        fork_failed.fork_result = Err("unavailable");
        handle_fork_command(&mut fork_failed).await;
        assert_eq!(
            fork_failed.operations,
            [
                "fork:session-1:Fork: session-1",
                "error:Failed to fork session: unavailable"
            ]
        );

        let mut switch_failed = host();
        switch_failed.switch_result = Err("closed");
        handle_fork_command(&mut switch_failed).await;
        assert!(switch_failed.operations.last().is_some_and(
            |operation| operation == "error:Failed to switch to forked session: closed"
        ));
    }

    struct ExportHost {
        session_id: Option<String>,
        work_dir: PathBuf,
        context: Result<SessionExportContext, &'static str>,
        operations: Vec<String>,
    }

    impl ExportMdCommandHost for ExportHost {
        type Error = &'static str;

        fn active_session_id(&self) -> Option<&str> {
            self.session_id.as_deref()
        }
        fn work_dir(&self) -> &Path {
            &self.work_dir
        }

        async fn get_session_context(&mut self) -> Result<SessionExportContext, Self::Error> {
            self.operations.push("context".to_owned());
            self.context.clone()
        }

        fn show_status(&mut self, message: &str) {
            self.operations.push(format!("status:{message}"));
        }
        fn show_error(&mut self, message: &str) {
            self.operations.push(format!("error:{message}"));
        }
        fn show_notice(&mut self, title: &str, body: &str) {
            self.operations.push(format!("notice:{title}:{body}"));
        }
    }

    fn export_context() -> SessionExportContext {
        SessionExportContext {
            history: vec![ContextMessage {
                role: ContextMessageRole::User,
                content: vec![ContentPart::Text {
                    text: "hello".to_owned(),
                }],
                tool_calls: Vec::new(),
                tool_call_id: None,
                origin: None,
            }],
            token_count: 12,
        }
    }

    fn export_host(work_dir: PathBuf) -> ExportHost {
        ExportHost {
            session_id: Some("session-123456".to_owned()),
            work_dir,
            context: Ok(export_context()),
            operations: Vec::new(),
        }
    }

    #[tokio::test]
    async fn exports_default_markdown_path_and_reports_terminal_link() {
        let directory =
            std::env::temp_dir().join(format!("kimi-export-test-{}", uuid::Uuid::new_v4()));
        let mut host = export_host(directory.clone());
        let now = Utc
            .with_ymd_and_hms(2026, 7, 21, 10, 11, 12)
            .single()
            .expect("valid date");
        handle_export_md_command_at(&mut host, "", now).await;

        let output = directory.join("kimi-export-session--20260721-101112.md");
        let text = fs::read_to_string(&output).await.expect("export exists");
        assert!(text.contains("session_id: session-123456"));
        assert!(text.contains("hello"));
        assert_eq!(host.operations[0], "status:Exporting session as Markdown…");
        assert_eq!(host.operations[1], "context");
        assert!(host.operations[2].starts_with("notice:Exported 1 messages:\u{1b}]8;;file:"));
        fs::remove_dir_all(directory)
            .await
            .expect("remove temp export");
    }

    #[tokio::test]
    async fn handles_missing_session_empty_history_and_context_failure() {
        let directory = std::env::temp_dir();
        let now = Utc
            .with_ymd_and_hms(2026, 7, 21, 10, 11, 12)
            .single()
            .expect("valid date");

        let mut missing = export_host(directory.clone());
        missing.session_id = None;
        handle_export_md_command_at(&mut missing, "", now).await;
        assert_eq!(
            missing.operations,
            [format!("error:{NO_ACTIVE_SESSION_MESSAGE}")]
        );

        let mut empty = export_host(directory.clone());
        empty.context = Ok(SessionExportContext {
            history: Vec::new(),
            token_count: 0,
        });
        handle_export_md_command_at(&mut empty, "", now).await;
        assert_eq!(
            empty.operations.last().map(String::as_str),
            Some("error:No messages to export.")
        );

        let mut failed = export_host(directory);
        failed.context = Err("unavailable");
        handle_export_md_command_at(&mut failed, "", now).await;
        assert_eq!(
            failed.operations.last().map(String::as_str),
            Some("error:Failed to export session: unavailable")
        );
    }
}
