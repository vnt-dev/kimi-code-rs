use std::{fmt::Display, future::Future};

use super::swarm::LLM_NOT_SET_MESSAGE;

pub trait BtwCommandHost {
    type Error: Display + Send;

    fn model(&self) -> &str;
    fn has_session(&self) -> bool;
    fn close_or_cancel_btw_panel(&mut self);
    fn start_btw(&mut self) -> impl Future<Output = Result<String, Self::Error>> + Send;
    fn open_btw_panel(&mut self, agent_id: &str, prompt: &str);
    fn show_error(&mut self, message: &str);
}

/// Original:
///   apps/kimi-code/src/tui/commands/btw.ts
///   handleBtwCommand()
pub async fn handle_btw_command<H: BtwCommandHost>(host: &mut H, args: &str) {
    let prompt = args.trim();
    if host.model().trim().is_empty() || !host.has_session() {
        host.show_error(LLM_NOT_SET_MESSAGE);
        return;
    }

    host.close_or_cancel_btw_panel();
    match host.start_btw().await {
        Ok(agent_id) => host.open_btw_panel(&agent_id, prompt),
        Err(error) => host.show_error(&format!("Failed to start /btw: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Host {
        model: String,
        has_session: bool,
        result: Result<String, &'static str>,
        operations: Vec<String>,
    }

    impl BtwCommandHost for Host {
        type Error = &'static str;

        fn model(&self) -> &str {
            &self.model
        }

        fn has_session(&self) -> bool {
            self.has_session
        }

        fn close_or_cancel_btw_panel(&mut self) {
            self.operations.push("close".to_owned());
        }

        async fn start_btw(&mut self) -> Result<String, Self::Error> {
            self.operations.push("start".to_owned());
            self.result.clone()
        }

        fn open_btw_panel(&mut self, agent_id: &str, prompt: &str) {
            self.operations.push(format!("open:{agent_id}:{prompt}"));
        }

        fn show_error(&mut self, message: &str) {
            self.operations.push(format!("error:{message}"));
        }
    }

    fn host(result: Result<&str, &'static str>) -> Host {
        Host {
            model: "kimi-model".to_owned(),
            has_session: true,
            result: result.map(str::to_owned),
            operations: Vec::new(),
        }
    }

    #[tokio::test]
    async fn validates_model_and_session_before_touching_panel() {
        let mut missing_model = host(Ok("agent-1"));
        missing_model.model = "  ".to_owned();
        handle_btw_command(&mut missing_model, "question").await;
        assert_eq!(
            missing_model.operations,
            [format!("error:{LLM_NOT_SET_MESSAGE}")]
        );

        let mut missing_session = host(Ok("agent-1"));
        missing_session.has_session = false;
        handle_btw_command(&mut missing_session, "question").await;
        assert_eq!(
            missing_session.operations,
            [format!("error:{LLM_NOT_SET_MESSAGE}")]
        );
    }

    #[tokio::test]
    async fn closes_previous_panel_then_starts_and_opens_trimmed_prompt() {
        let mut host = host(Ok("agent-1"));
        handle_btw_command(&mut host, "  what are you doing?  ").await;
        assert_eq!(
            host.operations,
            ["close", "start", "open:agent-1:what are you doing?"]
        );
    }

    #[tokio::test]
    async fn reports_start_failure_without_opening_panel() {
        let mut host = host(Err("unavailable"));
        handle_btw_command(&mut host, "question").await;
        assert_eq!(
            host.operations,
            ["close", "start", "error:Failed to start /btw: unavailable"]
        );
    }
}
