//! `SYSTEM.md` main-agent prompt override.
//!
//! Original: `packages/agent-core-v2/src/app/agentFileCatalog/systemFile.ts`.

use std::{path::Path, sync::Arc};

use crate::{
    app::agent_profile_catalog::{
        AgentProfile, AgentProfileContext, DEFAULT_AGENT_PROFILE_NAME, SkillActiveOptions,
        render_prompt_template, skill_active_for,
    },
    os::interface::{
        host_file_system::HostFileSystemService,
        host_fs_errors::{HostFsError, OS_FS_UNAVAILABLE},
    },
};

use super::is_file_path;

pub const SYSTEM_MD_FILENAME: &str = "SYSTEM.md";

// Original: loadSystemMdProfile(). `Arc<AgentProfile>` is the Rust ownership
// adaptation: the returned closure must keep the builtin base prompt alive.
pub async fn load_system_md_profile(
    fs: &dyn HostFileSystemService,
    brand_home: &Path,
    builtin_default: Arc<AgentProfile>,
    warn: &(dyn Fn(&str) + Send + Sync),
) -> Result<Option<Arc<AgentProfile>>, HostFsError> {
    let path = brand_home.join(SYSTEM_MD_FILENAME);
    let text = match is_file_path(fs, &path).await {
        Ok(false) => return Ok(None),
        Ok(true) => match fs.read_text(&path, None).await {
            Ok(text) => text,
            Err(error) if is_unavailable(&error) => return Err(error),
            Err(error) => {
                warn(&format!(
                    "agent SYSTEM.md load failed: {error} [{}]",
                    path.display()
                ));
                return Ok(None);
            }
        },
        Err(error) if is_unavailable(&error) => return Err(error),
        Err(error) => {
            warn(&format!(
                "agent SYSTEM.md load failed: {error} [{}]",
                path.display()
            ));
            return Ok(None);
        }
    };
    if text.trim().is_empty() {
        return Ok(None);
    }

    let skill_active = builtin_default
        .tools
        .as_ref()
        .is_none_or(|tools| skill_active_for(tools))
        && !builtin_default
            .disallowed_tools
            .as_ref()
            .is_some_and(|tools| tools.iter().any(|tool| tool == "Skill"));
    let base = Arc::clone(&builtin_default);
    let system_prompt = Arc::new(move |context: &AgentProfileContext| {
        render_prompt_template(
            &text,
            context,
            SkillActiveOptions { skill_active },
            Some(base.system_prompt.as_ref()),
        )
    });
    Ok(Some(Arc::new(AgentProfile {
        name: DEFAULT_AGENT_PROFILE_NAME.into(),
        description: builtin_default.description.clone(),
        when_to_use: None,
        is_override: Some(true),
        tools: builtin_default.tools.clone(),
        disallowed_tools: builtin_default.disallowed_tools.clone(),
        subagents: builtin_default.subagents.clone(),
        system_prompt,
        prompt_prefix: None,
        summary_policy: None,
    })))
}

fn is_unavailable(error: &HostFsError) -> bool {
    error.code() == OS_FS_UNAVAILABLE
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{
        app::agent_profile_catalog::AgentSystemPrompt,
        os::backends::node_local::host_fs_service::HostFileSystem,
    };

    use super::*;

    fn builtin_profile() -> Arc<AgentProfile> {
        let prompt: AgentSystemPrompt = Arc::new(|_| "BUILTIN".into());
        Arc::new(AgentProfile {
            name: DEFAULT_AGENT_PROFILE_NAME.into(),
            description: Some("builtin description".into()),
            when_to_use: Some("unused by SYSTEM.md".into()),
            is_override: Some(false),
            tools: Some(vec!["Read".into(), "Skill".into()]),
            disallowed_tools: None,
            subagents: Some(vec!["explore".into()]),
            system_prompt: prompt,
            prompt_prefix: None,
            summary_policy: None,
        })
    }

    #[tokio::test]
    async fn system_file_replaces_only_the_default_prompt_and_keeps_base_lazy() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home =
            std::env::temp_dir().join(format!("kimi-system-file-{}-{nonce}", std::process::id()));
        tokio::fs::create_dir_all(&home).await.unwrap();
        tokio::fs::write(
            home.join(SYSTEM_MD_FILENAME),
            "prefix ${base_prompt}${skills_section}",
        )
        .await
        .unwrap();

        let profile = load_system_md_profile(&HostFileSystem, &home, builtin_profile(), &|_| {})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(profile.name, DEFAULT_AGENT_PROFILE_NAME);
        assert_eq!(profile.description.as_deref(), Some("builtin description"));
        assert_eq!(profile.when_to_use, None);
        assert_eq!(profile.is_override, Some(true));
        let rendered = profile.render_system_prompt(&AgentProfileContext {
            skills: Some("- test".into()),
            now: Some("now".into()),
            ..AgentProfileContext::default()
        });
        assert!(rendered.starts_with("prefix BUILTIN"));
        assert!(rendered.contains("# Skills"));
        assert!(rendered.contains("- test"));

        assert!(
            load_system_md_profile(
                &HostFileSystem,
                Path::new("/definitely-missing-agent-system-file"),
                builtin_profile(),
                &|_| {}
            )
            .await
            .unwrap()
            .is_none()
        );
        tokio::fs::remove_dir_all(home).await.unwrap();
    }
}
