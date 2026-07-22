use std::{
    collections::HashMap,
    io,
    path::{Component, Path, PathBuf},
    sync::LazyLock,
    time::{SystemTime, UNIX_EPOCH},
};

use regex::Regex;
use serde_json::{Value, json};
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
    sync::Mutex,
};

use crate::agent_core_v2::_base::utils::workdir_slug::encode_work_dir_key;

const AGENT_WIRE_PROTOCOL_VERSION: &str = "1.4";
const MAX_TITLE_LENGTH: usize = 200;
const MAX_LAST_PROMPT_LENGTH: usize = 4000;

static PRIVATE_KEY_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)-----BEGIN [^-]*PRIVATE KEY-----[\s\S]*?-----END [^-]*PRIVATE KEY-----")
        .expect("private key pattern is valid")
});
static BEARER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(authorization)\s*:\s*bearer\s+\S+").expect("bearer pattern is valid")
});
static SECRET_ASSIGNMENT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b(api[_-]?key|token|secret|password|passwd|pwd)\b\s*[:=]\s*(?:"[^"]*"|'[^']*'|\S+)"#,
    )
    .expect("secret assignment pattern is valid")
});
static OPENAI_KEY_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bsk-[A-Za-z0-9_-]{12,}\b").expect("OpenAI key pattern is valid")
});
static LONG_TOKEN_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Za-z0-9][A-Za-z0-9+/=_-]{39,}\b").expect("token pattern is valid")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug)]
pub struct StoredPromptSession {
    pub id: String,
    pub work_dir: String,
    pub session_dir: PathBuf,
    pub model_alias: Option<String>,
    wire_path: PathBuf,
    append_lock: Mutex<()>,
}

impl StoredPromptSession {
    fn wire_path(&self) -> PathBuf {
        self.wire_path.clone()
    }

    // Original:
    //   packages/agent-core/src/agent/turn/index.ts
    //   TurnFlow.prompt(), TurnFlow.runOneTurn()
    //
    // The prompt record precedes the context record, matching agent-core. The
    // user message is durable before the provider request begins, so a failed
    // request can still be inspected and resumed like the source session.
    pub async fn append_user_prompt(&self, prompt: &str) -> io::Result<()> {
        let now = epoch_millis()?;
        let content = vec![json!({ "type": "text", "text": prompt })];
        self.append_records(&[
            json!({
                "type": "turn.prompt",
                "input": content,
                "origin": { "kind": "user" },
                "time": now,
            }),
            json!({
                "type": "context.append_message",
                "message": {
                    "role": "user",
                    "content": content,
                    "toolCalls": [],
                    "origin": { "kind": "user" },
                },
                "time": now,
            }),
        ])
        .await?;
        self.update_prompt_metadata(prompt).await
    }

    // Original:
    //   packages/agent-core/src/agent/context/index.ts
    //   Context.appendMessage()
    pub async fn append_assistant_message(&self, thinking: &str, content: &str) -> io::Result<()> {
        let mut parts = Vec::new();
        if !thinking.is_empty() {
            parts.push(json!({ "type": "think", "think": thinking }));
        }
        if !content.is_empty() {
            parts.push(json!({ "type": "text", "text": content }));
        }
        self.append_records(&[json!({
            "type": "context.append_message",
            "message": {
                "role": "assistant",
                "content": parts,
                "toolCalls": [],
            },
            "time": epoch_millis()?,
        })])
        .await
    }

    pub async fn load_history(&self) -> io::Result<Vec<StoredChatMessage>> {
        read_wire_history(&self.wire_path()).await
    }

    pub async fn append_model_alias(&self, model_alias: &str) -> io::Result<()> {
        self.append_records(&[json!({
            "type": "config.update",
            "modelAlias": model_alias,
            "time": epoch_millis()?,
        })])
        .await
    }

    async fn append_records(&self, records: &[Value]) -> io::Result<()> {
        let _guard = self.append_lock.lock().await;
        let wire_path = self.wire_path();
        if let Some(parent) = wire_path.parent() {
            create_private_dir(parent).await?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&wire_path)
            .await?;
        for record in records {
            let mut line = serde_json::to_vec(record).map_err(invalid_data)?;
            line.push(b'\n');
            file.write_all(&line).await?;
        }
        file.sync_all().await
    }

    async fn update_prompt_metadata(&self, prompt: &str) -> io::Result<()> {
        let state_path = self.session_dir.join("state.json");
        let raw = fs::read(&state_path).await?;
        let mut state: Value = serde_json::from_slice(&raw).map_err(invalid_data)?;
        let state = state.as_object_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "state.json is not an object")
        })?;
        let prompt = sanitize_prompt_metadata(prompt);
        if prompt.is_empty() {
            return Ok(());
        }
        state.insert("lastPrompt".to_owned(), Value::String(prompt.clone()));
        state.insert("updatedAt".to_owned(), Value::String(timestamp()?));
        let untitled = state
            .get("title")
            .and_then(Value::as_str)
            .is_none_or(|title| title.trim().is_empty() || title == "New Session");
        let custom = state
            .get("isCustomTitle")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if untitled && !custom {
            state.insert(
                "title".to_owned(),
                Value::String(prompt.chars().take(MAX_TITLE_LENGTH).collect()),
            );
            state.insert("isCustomTitle".to_owned(), Value::Bool(false));
        }
        write_json(&state_path, &Value::Object(state.clone())).await
    }
}

#[derive(Debug, Clone)]
pub struct PromptSessionStore {
    home_dir: PathBuf,
    sessions_dir: PathBuf,
}

impl PromptSessionStore {
    pub fn new(home_dir: impl Into<PathBuf>) -> Self {
        let home_dir = home_dir.into();
        let sessions_dir = home_dir.join("sessions");
        Self {
            home_dir,
            sessions_dir,
        }
    }

    // Original:
    //   packages/agent-core/src/session/store/session-store.ts
    //   SessionStore.create()
    //   packages/agent-core/src/rpc/core-impl.ts createSessionId()
    pub async fn create(
        &self,
        work_dir: &str,
        model_alias: &str,
    ) -> io::Result<StoredPromptSession> {
        let work_dir = normalize_work_dir(work_dir)?;
        let id = format!("session_{}", uuid::Uuid::new_v4());
        let session_dir = self
            .sessions_dir
            .join(encode_work_dir_key(&work_dir))
            .join(&id);
        create_private_dir(&session_dir).await?;
        let agent_dir = session_dir.join("agents/main");
        create_private_dir(&agent_dir).await?;

        let now = timestamp()?;
        write_json(
            &session_dir.join("state.json"),
            &json!({
                "createdAt": now,
                "updatedAt": now,
                "title": "New Session",
                "isCustomTitle": false,
                "workDir": work_dir,
                "agents": {
                    "main": {
                        "homedir": agent_dir.to_string_lossy(),
                        "type": "main",
                        "parentAgentId": null,
                    }
                },
                "custom": {},
            }),
        )
        .await?;

        append_json_line(
            &self.home_dir.join("session_index.jsonl"),
            &json!({
                "sessionId": id,
                "sessionDir": session_dir.to_string_lossy(),
                "workDir": work_dir,
            }),
        )
        .await?;

        let session = StoredPromptSession {
            id,
            work_dir,
            session_dir,
            model_alias: Some(model_alias.to_owned()),
            wire_path: agent_dir.join("wire.jsonl"),
            append_lock: Mutex::new(()),
        };
        session
            .append_records(&[
                json!({
                    "type": "metadata",
                    "protocol_version": AGENT_WIRE_PROTOCOL_VERSION,
                    "created_at": epoch_millis()?,
                }),
                json!({
                    "type": "config.update",
                    "modelAlias": model_alias,
                    "time": epoch_millis()?,
                }),
            ])
            .await?;
        Ok(session)
    }

    // Original:
    //   packages/agent-core/src/session/store/session-store.ts
    //   SessionStore.list(), SessionStore.get()
    pub async fn find_by_id(&self, id: &str) -> io::Result<Option<StoredPromptSession>> {
        if !is_safe_session_id(id) {
            return Ok(None);
        }
        let entries = self.read_index().await?;
        let Some(entry) = entries.get(id) else {
            return Ok(None);
        };
        self.open_entry(entry).await.map(Some)
    }

    pub async fn latest_for_work_dir(
        &self,
        work_dir: &str,
    ) -> io::Result<Option<StoredPromptSession>> {
        let work_dir = normalize_work_dir(work_dir)?;
        let entries = self.read_index().await?;
        let mut matches = Vec::new();
        for entry in entries.values() {
            let session = match self.open_entry(entry).await {
                Ok(session) if same_work_dir(&session.work_dir, &work_dir) => session,
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let state_modified = modified_or_epoch(&session.session_dir.join("state.json")).await?;
            let wire_modified = modified_or_epoch(&session.wire_path()).await?;
            let modified = state_modified.max(wire_modified);
            matches.push((modified, session));
        }
        matches.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.id.cmp(&right.1.id))
        });
        Ok(matches.into_iter().next().map(|(_, session)| session))
    }

    async fn open_entry(&self, entry: &SessionIndexEntry) -> io::Result<StoredPromptSession> {
        let raw = fs::read(entry.session_dir.join("state.json")).await?;
        let state: Value = serde_json::from_slice(&raw).map_err(invalid_data)?;
        let work_dir = state
            .get("workDir")
            .and_then(Value::as_str)
            .unwrap_or(&entry.work_dir)
            .to_owned();
        let current_wire_path = entry.session_dir.join("agents/main/wire.jsonl");
        let legacy_wire_path = entry.session_dir.join("wire.jsonl");
        let wire_path = if fs::try_exists(&current_wire_path).await?
            || !fs::try_exists(&legacy_wire_path).await?
        {
            current_wire_path
        } else {
            legacy_wire_path
        };
        let model_alias = read_model_alias(&wire_path).await?;
        Ok(StoredPromptSession {
            id: entry.id.clone(),
            work_dir,
            session_dir: entry.session_dir.clone(),
            model_alias,
            wire_path,
            append_lock: Mutex::new(()),
        })
    }

    async fn read_index(&self) -> io::Result<HashMap<String, SessionIndexEntry>> {
        let raw = match fs::read_to_string(self.home_dir.join("session_index.jsonl")).await {
            Ok(raw) => raw,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
            Err(error) => return Err(error),
        };
        let sessions_root = normalize_path(&self.sessions_dir)?;
        let mut entries = HashMap::new();
        for line in raw.lines().filter(|line| !line.trim().is_empty()) {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(id) = value.get("sessionId").and_then(Value::as_str) else {
                continue;
            };
            if value.get("deleted").and_then(Value::as_bool) == Some(true) {
                entries.remove(id);
                continue;
            }
            let (Some(session_dir), Some(work_dir)) = (
                value.get("sessionDir").and_then(Value::as_str),
                value.get("workDir").and_then(Value::as_str),
            ) else {
                continue;
            };
            let session_dir = PathBuf::from(session_dir);
            let Ok(normalized) = normalize_path(&session_dir) else {
                continue;
            };
            if !normalized.starts_with(&sessions_root)
                || normalized.file_name().and_then(|name| name.to_str()) != Some(id)
            {
                continue;
            }
            entries.insert(
                id.to_owned(),
                SessionIndexEntry {
                    id: id.to_owned(),
                    session_dir: normalized,
                    work_dir: work_dir.to_owned(),
                },
            );
        }
        Ok(entries)
    }
}

#[derive(Debug)]
struct SessionIndexEntry {
    id: String,
    session_dir: PathBuf,
    work_dir: String,
}

async fn read_wire_history(path: &Path) -> io::Result<Vec<StoredChatMessage>> {
    let raw = match fs::read_to_string(path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let ends_with_newline = raw.ends_with('\n');
    let mut history = Vec::new();
    let lines = raw.split_terminator('\n').collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(_) if !ends_with_newline && index + 1 == lines.len() => break,
            Err(error) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "wire.jsonl: corrupted line {} in {}: {error}",
                        index + 1,
                        path.display()
                    ),
                ));
            }
        };
        if value.get("type").and_then(Value::as_str) != Some("context.append_message") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let Some(role @ ("system" | "user" | "assistant")) =
            message.get("role").and_then(Value::as_str)
        else {
            continue;
        };
        let content = message
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|part| {
                (part.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| part.get("text").and_then(Value::as_str))
                    .flatten()
            })
            .collect::<Vec<_>>()
            .join("");
        if !content.is_empty() {
            history.push(StoredChatMessage {
                role: role.to_owned(),
                content,
            });
        }
    }
    Ok(history)
}

async fn read_model_alias(path: &Path) -> io::Result<Option<String>> {
    let raw = match fs::read_to_string(path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut alias = None;
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) == Some("config.update")
            && let Some(value) = value.get("modelAlias").and_then(Value::as_str)
        {
            alias = Some(value.to_owned());
        }
    }
    Ok(alias)
}

async fn append_json_line(path: &Path, value: &Value) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent).await?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    let mut line = serde_json::to_vec(value).map_err(invalid_data)?;
    line.push(b'\n');
    file.write_all(&line).await?;
    file.sync_all().await
}

async fn write_json(path: &Path, value: &Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(invalid_data)?;
    bytes.push(b'\n');
    fs::write(path, bytes).await
}

async fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await?;
    }
    Ok(())
}

fn normalize_work_dir(work_dir: &str) -> io::Result<String> {
    Ok(normalize_path(Path::new(work_dir))?
        .to_string_lossy()
        .into_owned())
}

fn normalize_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
        }
    }
    Ok(normalized)
}

fn is_safe_session_id(id: &str) -> bool {
    !matches!(id, "" | "." | "..")
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn same_work_dir(left: &str, right: &str) -> bool {
    normalize_work_dir(left).ok() == normalize_work_dir(right).ok()
}

fn sanitize_prompt_metadata(prompt: &str) -> String {
    let sanitized = PRIVATE_KEY_PATTERN.replace_all(prompt, "[redacted]");
    let sanitized = BEARER_PATTERN.replace_all(&sanitized, "$1: Bearer [redacted]");
    let sanitized = SECRET_ASSIGNMENT_PATTERN.replace_all(&sanitized, "$1=[redacted]");
    let sanitized = OPENAI_KEY_PATTERN.replace_all(&sanitized, "[redacted]");
    let sanitized = LONG_TOKEN_PATTERN.replace_all(&sanitized, "[redacted]");
    sanitized
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_LAST_PROMPT_LENGTH)
        .collect()
}

async fn modified_or_epoch(path: &Path) -> io::Result<SystemTime> {
    match fs::metadata(path).await {
        Ok(metadata) => Ok(metadata.modified().unwrap_or(UNIX_EPOCH)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(UNIX_EPOCH),
        Err(error) => Err(error),
    }
}

fn timestamp() -> io::Result<String> {
    Ok(chrono::DateTime::<chrono::Utc>::from(SystemTime::now()).to_rfc3339())
}

fn epoch_millis() -> io::Result<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .map_err(io::Error::other)
}

fn invalid_data(error: impl std::error::Error + Send + Sync + 'static) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home() -> PathBuf {
        std::env::temp_dir().join(format!("kimi-rust-prompt-store-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn prompt_metadata_redacts_source_secret_patterns() {
        assert_eq!(
            sanitize_prompt_metadata(
                " Authorization: Bearer abc token='value' sk-abcdefghijklmnop "
            ),
            "Authorization: Bearer [redacted] token=[redacted] [redacted]"
        );
    }

    #[tokio::test]
    async fn creates_source_compatible_session_and_restores_history() {
        let home = temp_home();
        let work_dir = home.join("项目 workspace");
        fs::create_dir_all(&work_dir).await.expect("create workdir");
        let store = PromptSessionStore::new(&home);
        let session = store
            .create(work_dir.to_str().expect("utf8 workdir"), "local/test")
            .await
            .expect("create session");

        assert!(session.id.starts_with("session_"));
        assert_eq!(
            session
                .session_dir
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap(),
            encode_work_dir_key(work_dir.to_str().expect("utf8 workdir"))
        );
        session
            .append_user_prompt("hello")
            .await
            .expect("append prompt");
        session
            .append_assistant_message("plan", "world")
            .await
            .expect("append response");

        let restored = store
            .find_by_id(&session.id)
            .await
            .expect("find session")
            .expect("session exists");
        assert_eq!(restored.model_alias.as_deref(), Some("local/test"));
        assert_eq!(
            restored.load_history().await.expect("load history"),
            vec![
                StoredChatMessage {
                    role: "user".to_owned(),
                    content: "hello".to_owned(),
                },
                StoredChatMessage {
                    role: "assistant".to_owned(),
                    content: "world".to_owned(),
                },
            ]
        );
        fs::remove_dir_all(home).await.expect("remove home");
    }

    #[tokio::test]
    async fn latest_session_is_scoped_to_workdir() {
        let home = temp_home();
        let first_work_dir = home.join("first");
        let second_work_dir = home.join("second");
        fs::create_dir_all(&first_work_dir)
            .await
            .expect("first workdir");
        fs::create_dir_all(&second_work_dir)
            .await
            .expect("second workdir");
        let store = PromptSessionStore::new(&home);
        let first = store
            .create(first_work_dir.to_str().unwrap(), "model-a")
            .await
            .expect("first session");
        let second = store
            .create(second_work_dir.to_str().unwrap(), "model-b")
            .await
            .expect("second session");

        assert_eq!(
            store
                .latest_for_work_dir(first_work_dir.to_str().unwrap())
                .await
                .expect("latest")
                .expect("first exists")
                .id,
            first.id
        );
        assert_ne!(first.id, second.id);
        fs::remove_dir_all(home).await.expect("remove home");
    }

    #[tokio::test]
    async fn tolerates_truncated_final_wire_line_but_rejects_earlier_corruption() {
        let home = temp_home();
        let path = home.join("wire.jsonl");
        fs::create_dir_all(&home).await.expect("create home");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"context.append_message\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}\n",
                "{\"type\":\"context.append"
            ),
        )
        .await
        .expect("write truncated wire");
        assert_eq!(read_wire_history(&path).await.expect("tolerated").len(), 1);

        fs::write(&path, "not-json\n{}\n")
            .await
            .expect("write corrupt wire");
        let error = read_wire_history(&path)
            .await
            .expect_err("corruption rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        fs::remove_dir_all(home).await.expect("remove home");
    }
}
