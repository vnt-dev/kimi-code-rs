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

use super::core_api::{
    ActivatePluginCommandPayload, ActivateSkillPayload, PromptFilePart, PromptInputPart,
    PromptPayload,
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
    LazyLock::new(|| Regex::new(r"[\s\u{FEFF}]+").expect("valid whitespace pattern"));

pub fn title_from_prompt_metadata_text(text: &str) -> String {
    truncate_utf16(text, MAX_TITLE_LENGTH)
}
pub fn is_untitled(title: Option<&str>) -> bool {
    title.is_none_or(|title| trim_js_whitespace(title).is_empty() || title == "New Session")
}
pub fn prompt_metadata_text_from_payload(payload: &PromptPayload) -> Option<String> {
    let text = payload
        .input
        .iter()
        .filter_map(|part| match part {
            PromptInputPart::Content(part) => prompt_part_text(part),
            PromptInputPart::File(PromptFilePart::File { name, .. }) => {
                Some(format!("[file: {name}]"))
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    sanitize_and_truncate_prompt_text(&text, MAX_LAST_PROMPT_LENGTH)
}
pub fn prompt_metadata_text_from_content_parts(parts: &[ContentPart]) -> Option<String> {
    let text = parts
        .iter()
        .filter_map(prompt_part_text)
        .collect::<Vec<_>>()
        .join("\n");
    sanitize_and_truncate_prompt_text(&text, MAX_LAST_PROMPT_LENGTH)
}
pub fn prompt_metadata_text_from_skill(payload: &ActivateSkillPayload) -> Option<String> {
    let args = payload
        .args
        .as_deref()
        .map(trim_js_whitespace)
        .filter(|args| !args.is_empty());
    sanitize_and_truncate_prompt_text(
        &args.map_or_else(
            || format!("/{}", payload.name),
            |args| format!("/{} {args}", payload.name),
        ),
        MAX_LAST_PROMPT_LENGTH,
    )
}
pub fn prompt_metadata_text_from_plugin_command(
    payload: &ActivatePluginCommandPayload,
) -> Option<String> {
    let command = format!("/{}:{}", payload.plugin_id, payload.command_name);
    let args = payload
        .args
        .as_deref()
        .map(trim_js_whitespace)
        .filter(|args| !args.is_empty());
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
            (!trim_js_whitespace(&text).is_empty()).then_some(text)
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
    let text = WHITESPACE
        .replace_all(&text, " ")
        .trim_matches(' ')
        .to_owned();
    (!text.is_empty()).then(|| truncate_utf16(&text, max_length))
}

// JavaScript String.length and slice() count UTF-16 code units. Rust strings
// cannot contain the unpaired surrogate that slice() can produce at a split
// boundary, so the Rust adaptation stops before splitting a scalar value.
fn truncate_utf16(text: &str, max_length: usize) -> String {
    let mut code_units = 0;
    text.chars()
        .take_while(|character| {
            let next = code_units + character.len_utf16();
            if next > max_length {
                return false;
            }
            code_units = next;
            true
        })
        .collect()
}

fn trim_js_whitespace(text: &str) -> &str {
    text.trim_matches(is_js_whitespace)
}

fn is_js_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
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
        reads: AtomicUsize,
        fail_read: AtomicBool,
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
                reads: AtomicUsize::new(0),
                fail_read: AtomicBool::new(false),
                fail_update: AtomicBool::new(false),
            }
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("metadata update failed")]
    struct MetadataUpdateFailed;

    #[derive(Debug, thiserror::Error)]
    #[error("metadata read failed")]
    struct MetadataReadFailed;

    #[async_trait]
    impl SessionMetadataContract for StubMetadata {
        async fn ready(&self) -> Result<(), SessionMetadataError> {
            Ok(())
        }

        fn on_did_change_metadata(&self) -> Event<SessionMetadataChangedEvent> {
            Event::none()
        }

        async fn read(&self) -> Result<SessionMeta, SessionMetadataError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            if self.fail_read.load(Ordering::Acquire) {
                return Err(Box::new(MetadataReadFailed));
            }
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
    fn truncates_with_javascript_utf16_length_without_splitting_rust_strings() {
        assert_eq!(
            title_from_prompt_metadata_text(&"x".repeat(201))
                .chars()
                .count(),
            200
        );
        assert_eq!(
            title_from_prompt_metadata_text(&format!("{}😀tail", "x".repeat(199))),
            "x".repeat(199)
        );
        assert_eq!(
            sanitize_and_truncate_prompt_text("😀😀😀", 4).as_deref(),
            Some("😀😀")
        );
        assert_eq!(
            sanitize_and_truncate_prompt_text("abcdef", 3).as_deref(),
            Some("abc")
        );
    }

    #[test]
    fn redacts_every_source_secret_pattern_then_normalizes_whitespace() {
        let long_token = "A".repeat(40);
        let input = format!(
            "before
-----begin rsa private key-----
private material
-----end rsa private key-----
Authorization : bearer secret-value
api-key: \"quoted secret\"
sk-abcdefghijkl
{long_token}\u{0000}\u{FEFF}after"
        );

        assert_eq!(
            sanitize_and_truncate_prompt_text(&input, MAX_LAST_PROMPT_LENGTH).as_deref(),
            Some(
                "before [redacted] Authorization: Bearer [redacted] api-key=[redacted] \
                 [redacted] [redacted] after"
            )
        );
        assert_eq!(sanitize_and_truncate_prompt_text("\u{FEFF}\n", 10), None);
    }

    #[test]
    fn payload_and_content_parts_preserve_source_projection_order() {
        let payload = PromptPayload {
            input: vec![
                ContentPart::Think {
                    think: "private reasoning".into(),
                    encrypted: None,
                },
                ContentPart::Text {
                    text: concat!(
                        " hello ",
                        "<system>Image compressed to fit model limits: caption</system>",
                        " world "
                    )
                    .into(),
                },
                ContentPart::ImageUrl {
                    image_url: crate::kosong::contract::message::MediaUrl {
                        url: "image".into(),
                        id: None,
                    },
                },
                ContentPart::AudioUrl {
                    audio_url: crate::kosong::contract::message::MediaUrl {
                        url: "audio".into(),
                        id: None,
                    },
                },
                ContentPart::VideoUrl {
                    video_url: crate::kosong::contract::message::MediaUrl {
                        url: "video".into(),
                        id: None,
                    },
                },
            ]
            .into_iter()
            .map(PromptInputPart::from)
            .collect(),
            disabled_tools: Some(vec!["WriteFile".into()]),
            skills: Vec::new(),
        };

        assert_eq!(
            prompt_metadata_text_from_payload(&payload).as_deref(),
            Some("hello world [image] [audio] [video]")
        );
        let content = payload
            .input
            .iter()
            .filter_map(|part| match part {
                PromptInputPart::Content(part) => Some(part.clone()),
                PromptInputPart::File(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            prompt_metadata_text_from_content_parts(&content),
            prompt_metadata_text_from_payload(&payload)
        );
        assert_eq!(prompt_metadata_text_from_content_parts(&[]), None);
    }

    #[test]
    fn skill_and_plugin_command_helpers_accept_source_payloads() {
        assert_eq!(
            prompt_metadata_text_from_skill(&ActivateSkillPayload {
                name: "review".into(),
                args: None,
            })
            .as_deref(),
            Some("/review")
        );
        assert_eq!(
            prompt_metadata_text_from_skill(&ActivateSkillPayload {
                name: "review".into(),
                args: Some(" api_key='secret value' ".into()),
            })
            .as_deref(),
            Some("/review api_key=[redacted]")
        );
        assert_eq!(
            prompt_metadata_text_from_skill(&ActivateSkillPayload {
                name: "review".into(),
                args: Some("\u{FEFF}".into()),
            })
            .as_deref(),
            Some("/review")
        );
        assert_eq!(
            prompt_metadata_text_from_plugin_command(&ActivatePluginCommandPayload {
                plugin_id: "git".into(),
                command_name: "status".into(),
                args: Some(" --short ".into()),
            })
            .as_deref(),
            Some("/git:status --short")
        );
    }

    #[test]
    fn untitled_detection_matches_source_cases() {
        assert!(is_untitled(None));
        assert!(is_untitled(Some(" \t\n")));
        assert!(is_untitled(Some("\u{FEFF}")));
        assert!(is_untitled(Some("New Session")));
        assert!(!is_untitled(Some("\u{0085}")));
        assert!(!is_untitled(Some("new session")));
        assert!(!is_untitled(Some("Existing")));
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
        assert_eq!(metadata.reads.load(Ordering::Relaxed), 0);

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

    #[tokio::test]
    async fn metadata_read_failures_do_not_update_or_publish() {
        let metadata = StubMetadata::new(None, None);
        metadata.fail_read.store(true, Ordering::Release);
        let event_service = EventService::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        let _subscription = event_service.subscribe(Arc::new(move |event| {
            captured.lock().unwrap().push(event.clone());
        }));

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

        assert!(error.downcast_ref::<MetadataReadFailed>().is_some());
        assert!(metadata.patches.lock().unwrap().is_empty());
        assert!(events.lock().unwrap().is_empty());
    }
}
