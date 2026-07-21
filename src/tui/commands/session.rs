use std::{fmt::Display, future::Future};

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
}
