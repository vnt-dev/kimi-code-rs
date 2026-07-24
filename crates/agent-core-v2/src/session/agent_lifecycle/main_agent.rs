//! Main-agent lifecycle convenience helpers.
//!
//! Original: `session/agentLifecycle/mainAgent.ts`.

pub use super::contract::MAIN_AGENT_ID;

/// Minimal lifecycle surface needed by the conventional-main composition
/// helper. The concrete registry is migrated separately.
pub trait MainAgentLifecycle {
    type Agent;
    type Error;
    async fn create_main(&self) -> Result<Self::Agent, Self::Error>;
}

// Original: ensureMainAgent().
pub async fn ensure_main_agent<L: MainAgentLifecycle>(lifecycle: &L) -> Result<L::Agent, L::Error> {
    lifecycle.create_main().await
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Lifecycle;
    impl MainAgentLifecycle for Lifecycle {
        type Agent = &'static str;
        type Error = ();
        async fn create_main(&self) -> Result<Self::Agent, Self::Error> {
            Ok(MAIN_AGENT_ID)
        }
    }
    #[tokio::test]
    async fn creates_the_conventional_main_agent() {
        assert_eq!(ensure_main_agent(&Lifecycle).await, Ok("main"));
        assert_eq!(MAIN_AGENT_ID, "main");
    }
}
