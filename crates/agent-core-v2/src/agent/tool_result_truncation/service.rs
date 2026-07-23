use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::ServicesAccessorExt,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    agent::scope_context::{AGENT_SCOPE_CONTEXT_ID, AgentScopeContext},
    app::bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
    kosong::contract::message::ContentPart,
    persistence::interface::storage::{
        FILE_SYSTEM_STORAGE_SERVICE_ID, FileSystemStorageService, FileSystemStorageServiceHandle,
        StorageWriteOptions,
    },
    tool::tool_contract::{ExecutableToolOutput, ExecutableToolResult},
};

use super::contract::{
    AGENT_TOOL_RESULT_TRUNCATION_SERVICE_ID, AgentToolResultTruncationServiceContract,
    AgentToolResultTruncationServiceHandle, ToolResultTruncationInput,
};

const TOOL_RESULT_MAX_CHARS: usize = 50_000;
const TOOL_RESULT_PREVIEW_CHARS: usize = 2_000;

pub struct ToolResultTruncationService {
    home_dir: PathBuf,
    storage_scope: String,
    storage: Arc<dyn FileSystemStorageService>,
}

impl ToolResultTruncationService {
    // Original:
    //   packages/agent-core-v2/src/agent/toolResultTruncation/toolResultTruncationService.ts
    //   ToolResultTruncationService.constructor()
    pub fn new(
        bootstrap: BootstrapServiceHandle,
        agent: &AgentScopeContext,
        storage: FileSystemStorageServiceHandle,
    ) -> Self {
        Self {
            home_dir: bootstrap.home_dir().into(),
            storage_scope: agent.scope(Some("tool-results")),
            storage: Arc::clone(&storage.0),
        }
    }

    // Original: ToolResultTruncationService.saveToolResult().
    async fn save_tool_result(
        &self,
        tool_name: &str,
        tool_call_id: &str,
        text: &str,
    ) -> Option<PathBuf> {
        let key = format!(
            "{}-{}.txt",
            safe_tool_result_file_stem(tool_name, tool_call_id),
            Uuid::new_v4()
        );
        self.storage
            .write(
                &self.storage_scope,
                &key,
                text.as_bytes(),
                StorageWriteOptions { atomic: true },
            )
            .await
            .ok()?;
        Some(self.home_dir.join(&self.storage_scope).join(key))
    }
}

#[async_trait]
impl AgentToolResultTruncationServiceContract for ToolResultTruncationService {
    // Original: ToolResultTruncationService.truncateForModel().
    async fn truncate_for_model(&self, input: ToolResultTruncationInput) -> ExecutableToolResult {
        let Some(text) = persistable_tool_result_text(&input.result.output) else {
            return input.result;
        };
        if text.encode_utf16().count() <= TOOL_RESULT_MAX_CHARS
            || input.result.truncated == Some(true)
        {
            return input.result;
        }
        let Some(output_path) = self
            .save_tool_result(&input.tool_name, &input.tool_call_id, &text)
            .await
        else {
            return input.result;
        };
        ExecutableToolResult {
            output: ExecutableToolOutput::Text(render_persisted_tool_result(
                &input.tool_name,
                &input.tool_call_id,
                &text,
                &output_path.to_string_lossy(),
            )),
            truncated: Some(true),
            ..input.result
        }
    }
}

// Original: toolResultTruncationService.ts, persistableToolResultText().
fn persistable_tool_result_text(output: &ExecutableToolOutput) -> Option<String> {
    match output {
        ExecutableToolOutput::Text(text) => Some(text.clone()),
        ExecutableToolOutput::Content(parts) => {
            let mut output = String::new();
            for part in parts {
                let ContentPart::Text { text } = part else {
                    return None;
                };
                output.push_str(text);
            }
            Some(output)
        }
    }
}

// Original: toolResultTruncationService.ts, renderPersistedToolResult().
fn render_persisted_tool_result(
    tool_name: &str,
    tool_call_id: &str,
    text: &str,
    output_path: &str,
) -> String {
    let length = text.encode_utf16().count();
    let preview = utf16_prefix(text, TOOL_RESULT_PREVIEW_CHARS);
    [
        format!("Tool output exceeded {TOOL_RESULT_MAX_CHARS} characters; showing a preview only."),
        format!("tool_name: {tool_name}"),
        format!("tool_call_id: {tool_call_id}"),
        format!("output_size_chars: {length}"),
        format!("output_size_bytes: {}", text.len()),
        format!("output_path: {output_path}"),
        "next_step: Use Read with output_path to page through the full output.".into(),
        String::new(),
        "[preview]".into(),
        preview.into(),
    ]
    .join("\n")
}

fn utf16_prefix(text: &str, max_units: usize) -> &str {
    // JavaScript slice may end between a surrogate pair. Rust strings cannot
    // contain that isolated surrogate, so this adaptation stops before the
    // scalar only for that boundary case; all complete-codepoint slices match.
    let mut units = 0;
    let mut boundary = 0;
    for (index, character) in text.char_indices() {
        let next = units + character.len_utf16();
        if next > max_units {
            break;
        }
        units = next;
        boundary = index + character.len_utf8();
    }
    &text[..boundary]
}

// Original: toolResultTruncationService.ts, safeToolResultFileStem().
fn safe_tool_result_file_stem(tool_name: &str, tool_call_id: &str) -> String {
    let mut label = String::new();
    let mut replacing = false;
    for character in format!("{tool_name}-{tool_call_id}").chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            label.push(character);
            replacing = false;
        } else if !replacing {
            label.push('_');
            replacing = true;
        }
    }
    let label = label.trim_matches('_');
    let label = &label[..label.len().min(80)];
    if label.is_empty() {
        "tool-result".into()
    } else {
        label.into()
    }
}

// Original: registerScopedService(... ToolResultTruncationService ...).
pub fn register_tool_result_truncation_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_TOOL_RESULT_TRUNCATION_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            let agent = accessor.get(AGENT_SCOPE_CONTEXT_ID)?;
            let storage = accessor.get(FILE_SYSTEM_STORAGE_SERVICE_ID)?;
            let service: Arc<dyn AgentToolResultTruncationServiceContract> =
                Arc::new(ToolResultTruncationService::new(
                    (*bootstrap).clone(),
                    agent.as_ref(),
                    (*storage).clone(),
                ));
            Ok(AgentToolResultTruncationServiceHandle(service))
        }),
        InstantiationType::Eager,
        "toolResultTruncation",
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::{
        agent::scope_context::{AgentScopeContextInput, make_agent_scope_context},
        app::bootstrap::{BootstrapOptions, BootstrapService, BootstrapServiceContract},
        kosong::contract::message::MediaUrl,
        persistence::backends::node_fs::file_storage_service::FileStorageService,
    };

    fn service(home: &std::path::Path) -> ToolResultTruncationService {
        let bootstrap: Arc<dyn BootstrapServiceContract> =
            Arc::new(BootstrapService::new(BootstrapOptions {
                home_dir: home.into(),
                config_path: home.join("config.toml"),
                os_home_dir: home.into(),
                platform: "linux".into(),
                arch: "x64".into(),
                cwd: home.into(),
                env: HashMap::new(),
                client_version: "test".into(),
            }));
        let agent = make_agent_scope_context(AgentScopeContextInput {
            agent_id: "main".into(),
            agent_scope: "sessions/workspace/session/agents/main".into(),
        });
        let storage: Arc<dyn FileSystemStorageService> =
            Arc::new(FileStorageService::with_default_modes(home));
        ToolResultTruncationService::new(
            BootstrapServiceHandle(bootstrap),
            &agent,
            FileSystemStorageServiceHandle(storage),
        )
    }

    fn temporary_home() -> PathBuf {
        std::env::temp_dir().join(format!("kimi-tool-result-{}", Uuid::new_v4()))
    }

    #[tokio::test]
    async fn persists_oversized_utf16_text_and_preserves_result_fields() {
        let home = temporary_home();
        let service = service(&home);
        let text = format!("{}tail survives on disk", "😀".repeat(25_001));
        let result = service
            .truncate_for_model(ToolResultTruncationInput {
                tool_name: "Lookup Tool".into(),
                tool_call_id: "call:lookup".into(),
                result: ExecutableToolResult::error(text.clone()),
            })
            .await;
        assert!(result.is_error);
        assert_eq!(result.truncated, Some(true));
        let ExecutableToolOutput::Text(rendered) = result.output else {
            panic!("expected text preview")
        };
        assert!(rendered.contains("output_size_chars: 50023"));
        assert!(!rendered.contains("tail survives on disk"));
        assert_eq!(rendered.matches('😀').count(), 1_000);
        let path = rendered
            .lines()
            .find_map(|line| line.strip_prefix("output_path: "))
            .unwrap();
        assert!(path.contains("Lookup_Tool-call_lookup-"));
        assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), text);
        tokio::fs::remove_dir_all(home).await.unwrap();
    }

    #[tokio::test]
    async fn joins_text_parts_and_leaves_truncated_or_media_results_unchanged() {
        let home = temporary_home();
        let service = service(&home);
        let parts = vec![
            ContentPart::Text {
                text: "first\n".into(),
            },
            ContentPart::Text {
                text: "y".repeat(50_001),
            },
        ];
        let result = service
            .truncate_for_model(ToolResultTruncationInput {
                tool_name: "Lookup".into(),
                tool_call_id: "call_text".into(),
                result: ExecutableToolResult::success(ExecutableToolOutput::Content(parts)),
            })
            .await;
        assert_eq!(result.truncated, Some(true));

        let mut already = ExecutableToolResult::success("z".repeat(50_001));
        already.truncated = Some(true);
        assert_eq!(
            service
                .truncate_for_model(ToolResultTruncationInput {
                    tool_name: "Lookup".into(),
                    tool_call_id: "already".into(),
                    result: already.clone(),
                })
                .await,
            already
        );
        let media = ExecutableToolResult::success(ExecutableToolOutput::Content(vec![
            ContentPart::Text {
                text: "z".repeat(50_001),
            },
            ContentPart::ImageUrl {
                image_url: MediaUrl {
                    url: "file:///image.png".into(),
                    id: None,
                },
            },
        ]));
        assert_eq!(
            service
                .truncate_for_model(ToolResultTruncationInput {
                    tool_name: "Lookup".into(),
                    tool_call_id: "media".into(),
                    result: media.clone(),
                })
                .await,
            media
        );
        tokio::fs::remove_dir_all(home).await.unwrap();
    }

    #[test]
    fn sanitizes_file_stems_with_ascii_source_rules_and_limit() {
        // The inserted hyphen is allowed, so the source's fallback is not
        // reached even when both labels contain only replaced characters.
        assert_eq!(safe_tool_result_file_stem("!!!", "???"), "-");
        assert_eq!(
            safe_tool_result_file_stem("Lookup Tool", "call:id"),
            "Lookup_Tool-call_id"
        );
        assert_eq!(safe_tool_result_file_stem(&"a".repeat(90), "id").len(), 80);
    }
}
