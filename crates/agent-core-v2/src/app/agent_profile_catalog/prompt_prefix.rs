//! Best-effort per-invocation profile prompt prefixes.
//!
//! Original: `packages/agent-core-v2/src/app/agentProfileCatalog/promptPrefix.ts`.

use super::contract::{AgentProfile, AgentProfilePromptPrefixContext};

// Original: applyProfilePromptPrefix(). Callback errors and empty prefixes are
// deliberately swallowed, leaving the caller's prompt unchanged.
pub async fn apply_profile_prompt_prefix(
    profile: &AgentProfile,
    prompt: &str,
    context: AgentProfilePromptPrefixContext,
) -> String {
    let Some(prompt_prefix) = &profile.prompt_prefix else {
        return prompt.into();
    };
    match prompt_prefix(context).await {
        Ok(prefix) if !prefix.is_empty() => format!("{prefix}\n\n{prompt}"),
        Ok(_) | Err(_) => prompt.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::{io, sync::Arc};

    use async_trait::async_trait;

    use crate::session::process::{
        ProcessExecOptions, SessionProcess, SessionProcessRunnerContract,
        SessionProcessRunnerHandle, SessionProcessRunnerResult,
    };

    use super::*;
    use crate::app::agent_profile_catalog::contract::{AgentProfileContext, AgentPromptPrefix};

    struct UnusedRunner;

    #[async_trait]
    impl SessionProcessRunnerContract for UnusedRunner {
        async fn exec(
            &self,
            _: &[String],
            _: Option<ProcessExecOptions>,
        ) -> SessionProcessRunnerResult<SessionProcess> {
            unreachable!("prompt prefix test does not spawn processes")
        }
    }

    fn profile(prefix: Option<AgentPromptPrefix>) -> AgentProfile {
        AgentProfile {
            name: "test".into(),
            description: None,
            when_to_use: None,
            is_override: None,
            tools: None,
            disallowed_tools: None,
            subagents: None,
            model: None,
            system_prompt: Arc::new(|_: &AgentProfileContext| String::new()),
            prompt_prefix: prefix,
            summary_policy: None,
        }
    }

    fn context() -> AgentProfilePromptPrefixContext {
        AgentProfilePromptPrefixContext {
            cwd: "/repo".into(),
            runner: SessionProcessRunnerHandle(Arc::new(UnusedRunner)),
            log: None,
        }
    }

    #[tokio::test]
    async fn prefixes_only_nonempty_successful_results() {
        let successful = profile(Some(Arc::new(|_| {
            Box::pin(async { Ok("<git-context />".into()) })
        })));
        assert_eq!(
            apply_profile_prompt_prefix(&successful, "inspect", context()).await,
            "<git-context />\n\ninspect"
        );

        let empty = profile(Some(Arc::new(|_| Box::pin(async { Ok(String::new()) }))));
        assert_eq!(
            apply_profile_prompt_prefix(&empty, "inspect", context()).await,
            "inspect"
        );

        let failing = profile(Some(Arc::new(|_| {
            Box::pin(async {
                Err(
                    Box::new(io::Error::other("failed"))
                        as crate::app::agent_profile_catalog::contract::AgentPromptPrefixError,
                )
            })
        })));
        assert_eq!(
            apply_profile_prompt_prefix(&failing, "inspect", context()).await,
            "inspect"
        );
        assert_eq!(
            apply_profile_prompt_prefix(&profile(None), "inspect", context()).await,
            "inspect"
        );
    }
}
