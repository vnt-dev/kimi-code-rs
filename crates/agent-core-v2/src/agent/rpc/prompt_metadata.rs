//! Prompt title and last-prompt metadata helpers.
//!
//! Original: `agent/rpc/prompt-metadata.ts`.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

use crate::{
    agent::media::extract_image_compression_captions,
    app::event::{EventServiceContract, GlobalDomainEvent},
    kosong::contract::message::ContentPart,
    session::session_metadata::{SessionMetaPatch, SessionMetadataContract, SessionMetadataError},
};

pub const MAX_TITLE_LENGTH: usize = 200;
pub const MAX_LAST_PROMPT_LENGTH: usize = 4000;

pub struct PromptMetadataUpdateTarget<'a> {
    pub metadata: &'a dyn SessionMetadataContract,
    pub event_service: &'a dyn EventServiceContract,
    pub session_id: &'a str,
}

static PRIVATE_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)-----BEGIN [^-]*PRIVATE KEY-----.*?-----END [^-]*PRIVATE KEY-----")
        .expect("valid private-key pattern")
});
static AUTHORIZATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(authorization)\s*:\s*bearer\s+\S+").expect("valid authorization pattern")
});
static SECRET_ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(api[_-]?key|token|secret|password|passwd|pwd)\b\s*[:=]\s*(?:"[^"]*"|'[^']*'|\S+)"#).expect("valid secret pattern")
});
static OPENAI_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bsk-[A-Za-z0-9_-]{12,}\b").expect("valid OpenAI-key pattern"));
static LONG_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Za-z0-9][A-Za-z0-9+/=_-]{39,}\b").expect("valid token pattern")
});
static CONTROL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\p{Cc}+").expect("valid control pattern"));
static WHITESPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+").expect("valid whitespace pattern"));

pub fn title_from_prompt_metadata_text(text: &str) -> String {
    text.chars().take(MAX_TITLE_LENGTH).collect()
}
pub fn is_untitled(title: Option<&str>) -> bool {
    title.is_none_or(|title| title.trim().is_empty() || title == "New Session")
}
pub fn prompt_metadata_text_from_content_parts(parts: &[ContentPart]) -> Option<String> {
    let text = parts
        .iter()
        .filter_map(prompt_part_text)
        .collect::<Vec<_>>()
        .join("\n");
    sanitize_and_truncate_prompt_text(&text, MAX_LAST_PROMPT_LENGTH)
}
pub fn prompt_metadata_text_from_skill(name: &str, args: Option<&str>) -> Option<String> {
    let args = args.map(str::trim).filter(|args| !args.is_empty());
    sanitize_and_truncate_prompt_text(
        &args.map_or_else(|| format!("/{name}"), |args| format!("/{name} {args}")),
        MAX_LAST_PROMPT_LENGTH,
    )
}
pub fn prompt_metadata_text_from_plugin_command(
    plugin_id: &str,
    command_name: &str,
    args: Option<&str>,
) -> Option<String> {
    let command = format!("/{plugin_id}:{command_name}");
    let args = args.map(str::trim).filter(|args| !args.is_empty());
    sanitize_and_truncate_prompt_text(
        &args.map_or(command.clone(), |args| format!("{command} {args}")),
        MAX_LAST_PROMPT_LENGTH,
    )
}

// Original: applyPromptMetadataUpdate(). Metadata persistence completes
// before the live event is published; a failed update therefore emits
// nothing and remains visible to the caller.
pub async fn apply_prompt_metadata_update(
    target: PromptMetadataUpdateTarget<'_>,
    text: Option<&str>,
) -> Result<(), SessionMetadataError> {
    let Some(text) = text else {
        return Ok(());
    };
    let current = target.metadata.read().await?;
    let auto_title = (current.is_custom_title != Some(true)
        && is_untitled(current.title.as_deref()))
    .then(|| title_from_prompt_metadata_text(text));
    let is_custom_title = auto_title.as_ref().map(|_| false);

    target
        .metadata
        .update(SessionMetaPatch {
            title: auto_title.clone(),
            is_custom_title,
            last_prompt: Some(text.into()),
            ..SessionMetaPatch::default()
        })
        .await?;

    let mut patch = Map::from_iter([("lastPrompt".into(), Value::String(text.into()))]);
    if let Some(title) = &auto_title {
        patch.insert("title".into(), Value::String(title.clone()));
    }
    if let Some(is_custom_title) = is_custom_title {
        patch.insert("isCustomTitle".into(), Value::Bool(is_custom_title));
    }
    let mut payload = Map::from_iter([
        ("agentId".into(), Value::String("main".into())),
        ("sessionId".into(), Value::String(target.session_id.into())),
        ("patch".into(), Value::Object(patch)),
    ]);
    if let Some(title) = auto_title {
        payload.insert("title".into(), Value::String(title));
    }
    target.event_service.publish(GlobalDomainEvent {
        event_type: "session.meta.updated".into(),
        payload: Value::Object(payload),
    });
    Ok(())
}

fn prompt_part_text(part: &ContentPart) -> Option<String> {
    match part {
        ContentPart::Text { text } => {
            let text = extract_image_compression_captions(text).text;
            (!text.trim().is_empty()).then_some(text)
        }
        ContentPart::ImageUrl { .. } => Some("[image]".into()),
        ContentPart::AudioUrl { .. } => Some("[audio]".into()),
        ContentPart::VideoUrl { .. } => Some("[video]".into()),
        ContentPart::Think { .. } => None,
    }
}
pub fn sanitize_and_truncate_prompt_text(text: &str, max_length: usize) -> Option<String> {
    let text = PRIVATE_KEY.replace_all(text, "[redacted]");
    let text = AUTHORIZATION.replace_all(&text, "$1: Bearer [redacted]");
    let text = SECRET_ASSIGNMENT.replace_all(&text, "$1=[redacted]");
    let text = OPENAI_KEY.replace_all(&text, "[redacted]");
    let text = LONG_TOKEN.replace_all(&text, "[redacted]");
    let text = CONTROL.replace_all(&text, " ");
    let text = WHITESPACE.replace_all(&text, " ").trim().to_owned();
    (!text.is_empty()).then(|| text.chars().take(max_length).collect())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };

    use async_trait::async_trait;
    use serde_json::json;

    use crate::{
        _base::event::Event,
        app::event::EventService,
        session::session_metadata::{AgentMeta, SessionMeta, SessionMetadataChangedEvent},
    };

    use super::*;

    struct StubMetadata {
        data: Mutex<SessionMeta>,
        patches: Mutex<Vec<SessionMetaPatch>>,
        fail_update: AtomicBool,
    }

    impl StubMetadata {
        fn new(title: Option<&str>, is_custom_title: Option<bool>) -> Self {
            Self {
                data: Mutex::new(SessionMeta {
                    id: "s1".into(),
                    version: Some(2),
                    title: title.map(str::to_owned),
                    is_custom_title,
                    last_prompt: None,
                    created_at: 1,
                    updated_at: 1,
                    archived: false,
                    cwd: Some("/repo".into()),
                    forked_from: None,
                    agents: None,
                    custom: None,
                }),
                patches: Mutex::new(Vec::new()),
                fail_update: AtomicBool::new(false),
            }
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("metadata update failed")]
    struct MetadataUpdateFailed;

    #[async_trait]
    impl SessionMetadataContract for StubMetadata {
        async fn ready(&self) -> Result<(), SessionMetadataError> {
            Ok(())
        }

        fn on_did_change_metadata(&self) -> Event<SessionMetadataChangedEvent> {
            Event::none()
        }

        async fn read(&self) -> Result<SessionMeta, SessionMetadataError> {
            Ok(self.data.lock().unwrap().clone())
        }

        async fn update(&self, patch: SessionMetaPatch) -> Result<(), SessionMetadataError> {
            if self.fail_update.load(Ordering::Acquire) {
                return Err(Box::new(MetadataUpdateFailed));
            }
            let mut data = self.data.lock().unwrap();
            if let Some(title) = &patch.title {
                data.title = Some(title.clone());
            }
            if let Some(is_custom_title) = patch.is_custom_title {
                data.is_custom_title = Some(is_custom_title);
            }
            if let Some(last_prompt) = &patch.last_prompt {
                data.last_prompt = Some(last_prompt.clone());
            }
            drop(data);
            self.patches.lock().unwrap().push(patch);
            Ok(())
        }

        async fn set_title(&self, _title: String) -> Result<(), SessionMetadataError> {
            Ok(())
        }

        async fn set_archived(&self, _archived: bool) -> Result<(), SessionMetadataError> {
            Ok(())
        }

        async fn register_agent(
            &self,
            _agent_id: String,
            _meta: AgentMeta,
        ) -> Result<(), SessionMetadataError> {
            Ok(())
        }
    }

    #[test]
    fn extracts_redacts_and_truncates_prompt_metadata() {
        assert_eq!(
            title_from_prompt_metadata_text(&"x".repeat(201))
                .chars()
                .count(),
            200
        );
        assert_eq!(
            sanitize_and_truncate_prompt_text(
                "Authorization: Bearer secret-value\npassword=hello",
                4000
            )
            .as_deref(),
            Some("Authorization: Bearer [redacted] password=[redacted]")
        );
        assert_eq!(
            prompt_metadata_text_from_content_parts(&[
                ContentPart::Text {
                    text: " hello ".into()
                },
                ContentPart::ImageUrl {
                    image_url: crate::kosong::contract::message::MediaUrl {
                        url: "x".into(),
                        id: None
                    }
                }
            ])
            .as_deref(),
            Some("hello [image]")
        );
        assert!(is_untitled(Some("New Session")));
    }

    #[tokio::test]
    async fn metadata_update_sets_an_automatic_title_and_publishes_the_source_event() {
        let metadata = StubMetadata::new(None, None);
        let event_service = EventService::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        let _subscription = event_service.subscribe(Arc::new(move |event| {
            captured.lock().unwrap().push(event.clone());
        }));

        apply_prompt_metadata_update(
            PromptMetadataUpdateTarget {
                metadata: &metadata,
                event_service: &event_service,
                session_id: "s1",
            },
            Some("first prompt"),
        )
        .await
        .unwrap();

        assert_eq!(
            metadata.patches.lock().unwrap().as_slice(),
            [SessionMetaPatch {
                title: Some("first prompt".into()),
                is_custom_title: Some(false),
                last_prompt: Some("first prompt".into()),
                ..SessionMetaPatch::default()
            }]
        );
        assert_eq!(
            events.lock().unwrap().as_slice(),
            [GlobalDomainEvent {
                event_type: "session.meta.updated".into(),
                payload: json!({
                    "agentId": "main",
                    "sessionId": "s1",
                    "title": "first prompt",
                    "patch": {
                        "title": "first prompt",
                        "isCustomTitle": false,
                        "lastPrompt": "first prompt"
                    }
                }),
            }]
        );
    }

    #[tokio::test]
    async fn metadata_update_preserves_custom_titles_and_omits_undefined_event_fields() {
        let metadata = StubMetadata::new(Some("Custom"), Some(true));
        let event_service = EventService::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        let _subscription = event_service.subscribe(Arc::new(move |event| {
            captured.lock().unwrap().push(event.clone());
        }));

        apply_prompt_metadata_update(
            PromptMetadataUpdateTarget {
                metadata: &metadata,
                event_service: &event_service,
                session_id: "s1",
            },
            Some("another prompt"),
        )
        .await
        .unwrap();

        assert_eq!(
            metadata.patches.lock().unwrap().as_slice(),
            [SessionMetaPatch {
                last_prompt: Some("another prompt".into()),
                ..SessionMetaPatch::default()
            }]
        );
        assert_eq!(
            events.lock().unwrap()[0].payload,
            json!({
                "agentId": "main",
                "sessionId": "s1",
                "patch": {"lastPrompt": "another prompt"}
            })
        );
    }

    #[tokio::test]
    async fn absent_text_is_a_no_op_and_update_failures_do_not_publish() {
        let metadata = StubMetadata::new(None, None);
        let event_service = EventService::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        let _subscription = event_service.subscribe(Arc::new(move |event| {
            captured.lock().unwrap().push(event.clone());
        }));

        apply_prompt_metadata_update(
            PromptMetadataUpdateTarget {
                metadata: &metadata,
                event_service: &event_service,
                session_id: "s1",
            },
            None,
        )
        .await
        .unwrap();
        assert!(metadata.patches.lock().unwrap().is_empty());

        metadata.fail_update.store(true, Ordering::Release);
        let error = apply_prompt_metadata_update(
            PromptMetadataUpdateTarget {
                metadata: &metadata,
                event_service: &event_service,
                session_id: "s1",
            },
            Some("will fail"),
        )
        .await
        .unwrap_err();
        assert!(error.downcast_ref::<MetadataUpdateFailed>().is_some());
        assert!(events.lock().unwrap().is_empty());
    }
}
