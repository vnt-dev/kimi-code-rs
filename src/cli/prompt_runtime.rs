use std::{
    collections::HashMap,
    error::Error,
    fmt,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{Client, StatusCode, header::HeaderMap};
use serde_json::{Map, Value, json};

use super::{
    options::{CliOptions, resolve_output_format},
    prompt_render::{PromptOutput, write_resume_hint},
    prompt_session::{
        ApprovalHandler, CreateGoalInput, EventListener, PrintTurnAction, PromptEvent,
        PromptEventKind, PromptInput, PromptSession, PromptSessionError, QuestionHandler,
        Unsubscribe,
    },
    prompt_store::{PromptSessionStore, StoredChatMessage, StoredPromptSession},
    run_prompt::run_prompt_turn,
    sub::{
        provider::{ModelDefinition, ProviderDefinition},
        provider_config::ProviderConfigStore,
    },
    version::create_kimi_code_user_agent,
};
use crate::{
    oauth::{
        identity::{
            KimiHostIdentity, KimiIdentityOptions, create_kimi_default_headers,
            parse_kimi_code_custom_headers,
        },
        managed_auth::{
            KIMI_CODE_PROVIDER_NAME, ManagedKimiOAuthRefInput, OAuthStorageBackend,
            RuntimeAuthOptions, resolve_kimi_code_runtime_auth,
        },
        managed_usage::DEFAULT_KIMI_CODE_BASE_URL,
        toolkit::{
            KimiOAuthTokenRef, KimiOAuthToolkit, KimiOAuthToolkitOptions, NoManagedConfigAdapter,
        },
    },
    sdk::types::{CronTaskSnapshot, GoalSnapshot, PermissionMode, PromptPart, SessionStatus},
};

const DEFAULT_KIMI_BASE_URL: &str = "https://api.moonshot.ai/v1";
const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

#[derive(Debug)]
pub struct SystemPromptError {
    message: String,
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl SystemPromptError {
    fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    fn with_source(message: impl Into<String>, source: impl Error + Send + Sync + 'static) -> Self {
        Self {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }
}

impl fmt::Display for SystemPromptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for SystemPromptError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref().map(|source| source as _)
    }
}

#[derive(Debug, Clone)]
struct ChatCompletionTarget {
    alias: String,
    model: String,
    endpoint: String,
    api_key: String,
    headers: HeaderMap,
}

pub struct SystemPromptRuntime {
    data_dir: PathBuf,
    environment: HashMap<String, String>,
    client: Client,
    identity: Option<KimiHostIdentity>,
}

impl SystemPromptRuntime {
    pub fn new(
        data_dir: impl Into<PathBuf>,
        version: &str,
        environment: HashMap<String, String>,
    ) -> Result<Self, SystemPromptError> {
        let user_agent = create_kimi_code_user_agent(version)
            .map_err(|error| SystemPromptError::with_source("invalid Kimi user agent", error))?;
        let client = Client::builder()
            .user_agent(user_agent)
            .build()
            .map_err(|error| {
                SystemPromptError::with_source("failed to create HTTP client", error)
            })?;
        Ok(Self {
            data_dir: data_dir.into(),
            environment,
            client,
            identity: Some(KimiHostIdentity {
                user_agent_product: "kimi-code-cli".to_owned(),
                version: version.to_owned(),
                user_agent_suffix: None,
            }),
        })
    }

    #[cfg(test)]
    fn with_client(
        data_dir: impl Into<PathBuf>,
        environment: HashMap<String, String>,
        client: Client,
    ) -> Self {
        Self {
            data_dir: data_dir.into(),
            environment,
            client,
            identity: None,
        }
    }

    // Original:
    //   apps/kimi-code/src/cli/run-prompt.ts
    //   runPrompt()
    //
    // Rust adaptation:
    //   This first concrete process runtime connects the migrated CLI event
    //   renderer to OpenAI-compatible chat-completions providers. The source
    //   harness still owns tools, hooks, goals, and alternate wire protocols;
    //   those remain explicit migration boundaries below. Session storage and
    //   text history use the source wire format so either implementation can
    //   resume the resulting session.
    pub async fn run(
        &self,
        options: &CliOptions,
        stdout: &mut dyn PromptOutput,
        stderr: &mut dyn PromptOutput,
    ) -> Result<(), SystemPromptError> {
        if options
            .prompt
            .as_deref()
            .is_some_and(|prompt| prompt.trim_start().starts_with("/goal"))
        {
            return Err(SystemPromptError::message(
                "headless goals are not yet available in the Rust prompt runtime",
            ));
        }

        let store = ProviderConfigStore::new(self.data_dir.join("config.toml"));
        store.ensure_config_file().await.map_err(|error| {
            SystemPromptError::with_source("failed to create config.toml", error)
        })?;
        let config = store
            .get_config()
            .await
            .map_err(|error| SystemPromptError::with_source("failed to read config.toml", error))?;
        let work_dir = std::env::current_dir()
            .map_err(|error| SystemPromptError::with_source("failed to resolve cwd", error))?
            .to_string_lossy()
            .into_owned();
        let session_store = PromptSessionStore::new(&self.data_dir);
        let stored_session = if let Some(session_id) = options.session.as_deref() {
            let session = session_store
                .find_by_id(session_id)
                .await
                .map_err(|error| {
                    SystemPromptError::with_source(
                        format!("failed to load session \"{session_id}\""),
                        error,
                    )
                })?
                .ok_or_else(|| {
                    SystemPromptError::message(format!("Session \"{session_id}\" not found."))
                })?;
            if !same_runtime_work_dir(&session.work_dir, &work_dir) {
                stderr.write(&format!(
                    "Session \"{session_id}\" was created under a different directory.\n  cd \"{}\" && kimi -r {session_id}\n\n",
                    session.work_dir
                ));
                return Err(SystemPromptError::message(format!(
                    "Session \"{session_id}\" was created under a different directory."
                )));
            }
            Some(session)
        } else if options.continue_previous {
            let session = session_store
                .latest_for_work_dir(&work_dir)
                .await
                .map_err(|error| {
                    SystemPromptError::with_source("failed to list prompt sessions", error)
                })?;
            if session.is_none() {
                stderr.write(&format!(
                    "No sessions to continue under \"{work_dir}\"; starting a fresh session.\n"
                ));
            }
            session
        } else {
            None
        };
        let alias = options
            .model
            .as_deref()
            .or_else(|| {
                stored_session
                    .as_ref()
                    .and_then(|session| session.model_alias.as_deref())
            })
            .or(config.default_model.as_deref())
            .filter(|model| !model.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| {
                SystemPromptError::message(
                    "No model configured. Run `kimi` and use /login to sign in, then retry; or set default_model in config.toml.",
                )
            })?;
        let model = config.models.get(&alias).ok_or_else(|| {
            SystemPromptError::message(format!(
                "Configured model alias \"{alias}\" does not exist in config.toml."
            ))
        })?;
        let provider = config.providers.get(&model.provider).ok_or_else(|| {
            SystemPromptError::message(format!(
                "Model \"{alias}\" references missing provider \"{}\".",
                model.provider
            ))
        })?;
        let target = self
            .resolve_target(&alias, model, provider, &model.provider)
            .await?;
        let stored_session = match stored_session {
            Some(session) => {
                if session.model_alias.as_deref() != Some(alias.as_str()) {
                    session.append_model_alias(&alias).await.map_err(|error| {
                        SystemPromptError::with_source(
                            format!("failed to update session \"{}\" model", session.id),
                            error,
                        )
                    })?;
                }
                session
            }
            None => session_store
                .create(&work_dir, &alias)
                .await
                .map_err(|error| {
                    SystemPromptError::with_source("failed to create prompt session", error)
                })?,
        };
        let history = stored_session.load_history().await.map_err(|error| {
            SystemPromptError::with_source(
                format!(
                    "failed to restore session \"{}\" history",
                    stored_session.id
                ),
                error,
            )
        })?;
        let session: Arc<dyn PromptSession> = Arc::new(HttpPromptSession::new(
            self.client.clone(),
            Arc::new(stored_session),
            target,
            history,
        ));
        let output_format = resolve_output_format(options, &self.environment)
            .map_err(|error| SystemPromptError::with_source("invalid output format", error))?;
        run_prompt_turn(
            Arc::clone(&session),
            options.prompt.as_deref().ok_or_else(|| {
                SystemPromptError::message("prompt text is required in print mode")
            })?,
            output_format,
            stdout,
            stderr,
        )
        .await
        .map_err(|error| SystemPromptError {
            message: error.to_string(),
            source: Some(error),
        })?;
        write_resume_hint(session.id(), output_format, stdout, stderr);
        Ok(())
    }

    async fn resolve_target(
        &self,
        alias: &str,
        model: &ModelDefinition,
        provider: &ProviderDefinition,
        provider_id: &str,
    ) -> Result<ChatCompletionTarget, SystemPromptError> {
        if !matches!(provider.provider_type.as_str(), "openai" | "kimi") {
            return Err(SystemPromptError::message(format!(
                "Provider type \"{}\" is not yet supported by the Rust prompt runtime; currently supported: openai, kimi.",
                provider.provider_type
            )));
        }
        let base_url = resolved_base_url(provider_id, provider, &self.environment);
        let api_key = match provider
            .api_key
            .as_deref()
            .filter(|key| !key.is_empty())
            .or_else(|| provider_config_api_key(provider))
        {
            Some(key) => key.to_owned(),
            None if provider.oauth.is_some() => {
                self.resolve_oauth_token(provider_id, provider).await?
            }
            None => {
                return Err(SystemPromptError::message(format!(
                    "Provider \"{provider_id}\" has no API key or OAuth credential."
                )));
            }
        };
        let headers = configured_headers(
            provider,
            &self.environment,
            self.identity.as_ref().map(|identity| KimiIdentityOptions {
                home_dir: self.data_dir.clone(),
                host: identity.clone(),
            }),
        )?;
        Ok(ChatCompletionTarget {
            alias: alias.to_owned(),
            model: model.model.clone(),
            endpoint: format!("{}/chat/completions", base_url.trim_end_matches('/')),
            api_key,
            headers,
        })
    }

    async fn resolve_oauth_token(
        &self,
        provider_id: &str,
        provider: &ProviderDefinition,
    ) -> Result<String, SystemPromptError> {
        let oauth = provider.oauth.as_ref().and_then(Value::as_object);
        let configured_reference = ManagedKimiOAuthRefInput {
            storage: oauth
                .and_then(|oauth| oauth.get("storage"))
                .and_then(Value::as_str)
                .and_then(|storage| match storage {
                    "file" => Some(OAuthStorageBackend::File),
                    "keyring" => Some(OAuthStorageBackend::Keyring),
                    _ => None,
                }),
            key: oauth
                .and_then(|oauth| oauth.get("key"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            oauth_host: oauth
                .and_then(|oauth| oauth.get("oauthHost"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        };
        if configured_reference.storage == Some(OAuthStorageBackend::Keyring) {
            // MIGRATION-TODO:
            // Original: KimiOAuthToolkit selects the configured keyring backend.
            // Temporary behavior: fail explicitly instead of reading the file
            // credential store under the same key.
            // Completion condition: port the keyring TokenStorage adapter.
            return Err(SystemPromptError::message(
                "OAuth keyring storage is not yet supported by the Rust prompt runtime",
            ));
        }
        let reference = if provider_id == KIMI_CODE_PROVIDER_NAME {
            let resolved = resolve_kimi_code_runtime_auth(RuntimeAuthOptions {
                configured_base_url: provider.base_url.as_deref(),
                configured_oauth_ref: Some(&configured_reference),
                environment: &self.environment,
            });
            KimiOAuthTokenRef {
                key: Some(resolved.oauth_ref.key),
                oauth_host: resolved.oauth_ref.oauth_host,
            }
        } else {
            KimiOAuthTokenRef {
                key: configured_reference.key,
                oauth_host: configured_reference.oauth_host,
            }
        };
        let toolkit: KimiOAuthToolkit<NoManagedConfigAdapter> =
            KimiOAuthToolkit::new(KimiOAuthToolkitOptions {
                identity: self.identity.clone(),
                home_dir: Some(self.data_dir.clone()),
                ..KimiOAuthToolkitOptions::default()
            })
            .map_err(|error| {
                SystemPromptError::with_source("failed to initialize OAuth credentials", error)
            })?;
        toolkit
            .ensure_fresh(Some(provider_id), false, Some(&reference))
            .await
            .map_err(|error| {
                SystemPromptError::with_source(
                    format!("failed to resolve OAuth token for provider \"{provider_id}\""),
                    error,
                )
            })
    }
}

fn provider_config_api_key(provider: &ProviderDefinition) -> Option<&str> {
    let key = if provider.provider_type == "kimi" {
        "KIMI_API_KEY"
    } else {
        "OPENAI_API_KEY"
    };
    provider_config_environment_value(provider, key)
}

fn same_runtime_work_dir(left: &str, right: &str) -> bool {
    Path::new(left) == Path::new(right)
}

fn resolved_base_url<'a>(
    provider_id: &str,
    provider: &'a ProviderDefinition,
    environment: &'a HashMap<String, String>,
) -> &'a str {
    if provider_id == KIMI_CODE_PROVIDER_NAME
        && let Some(value) = environment
            .get("KIMI_CODE_BASE_URL")
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    {
        return value;
    }
    if let Some(value) = provider
        .base_url
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return value;
    }
    let name = if provider.provider_type == "kimi" {
        "KIMI_BASE_URL"
    } else {
        "OPENAI_BASE_URL"
    };
    provider_config_environment_value(provider, name).unwrap_or_else(|| {
        if provider_id == KIMI_CODE_PROVIDER_NAME {
            DEFAULT_KIMI_CODE_BASE_URL
        } else if provider.provider_type == "kimi" {
            DEFAULT_KIMI_BASE_URL
        } else {
            DEFAULT_OPENAI_BASE_URL
        }
    })
}

fn provider_config_environment_value<'a>(
    provider: &'a ProviderDefinition,
    name: &str,
) -> Option<&'a str> {
    provider
        .additional_fields
        .get("env")
        .and_then(Value::as_object)
        .and_then(|environment| environment.get(name))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn configured_headers(
    provider: &ProviderDefinition,
    environment: &HashMap<String, String>,
    identity: Option<KimiIdentityOptions>,
) -> Result<HeaderMap, SystemPromptError> {
    let mut headers = HeaderMap::new();
    insert_headers(
        &mut headers,
        parse_kimi_code_custom_headers(environment)
            .into_iter()
            .map(|(name, value)| (name, Value::String(value))),
    )?;
    if let Some(identity) = identity {
        let identity_headers = create_kimi_default_headers(&identity).map_err(|error| {
            SystemPromptError::with_source("failed to create Kimi identity headers", error)
        })?;
        let identity_headers = if provider.provider_type == "kimi" {
            identity_headers.into_iter().collect::<Vec<_>>()
        } else {
            identity_headers
                .into_iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
                .collect::<Vec<_>>()
        };
        insert_headers(
            &mut headers,
            identity_headers
                .into_iter()
                .map(|(name, value)| (name, Value::String(value))),
        )?;
    }
    for field in ["defaultHeaders", "customHeaders"] {
        if let Some(configured) = provider
            .additional_fields
            .get(field)
            .and_then(Value::as_object)
        {
            insert_headers(
                &mut headers,
                configured
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone())),
            )?;
        }
    }
    Ok(headers)
}

fn insert_headers(
    headers: &mut HeaderMap,
    configured: impl IntoIterator<Item = (String, Value)>,
) -> Result<(), SystemPromptError> {
    for (name, value) in configured {
        let Some(value) = value.as_str() else {
            return Err(SystemPromptError::message(format!(
                "Provider header \"{name}\" must be a string."
            )));
        };
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
            SystemPromptError::with_source(
                format!("Invalid provider header name \"{name}\""),
                error,
            )
        })?;
        let value = reqwest::header::HeaderValue::from_str(value).map_err(|error| {
            SystemPromptError::with_source("Invalid provider header value", error)
        })?;
        headers.insert(name, value);
    }
    Ok(())
}

struct ListenerRegistry {
    next_id: AtomicU64,
    listeners: Mutex<HashMap<u64, EventListener>>,
}

impl ListenerRegistry {
    fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            listeners: Mutex::new(HashMap::new()),
        }
    }

    fn emit(&self, event: PromptEvent) {
        let listeners = self
            .listeners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for listener in listeners {
            listener(event.clone());
        }
    }
}

struct HttpPromptSession {
    stored: Arc<StoredPromptSession>,
    client: Client,
    target: ChatCompletionTarget,
    history: Vec<StoredChatMessage>,
    listeners: Arc<ListenerRegistry>,
    permission: Mutex<PermissionMode>,
    approval_handler: Mutex<Option<ApprovalHandler>>,
    question_handler: Mutex<Option<QuestionHandler>>,
}

impl HttpPromptSession {
    fn new(
        client: Client,
        stored: Arc<StoredPromptSession>,
        target: ChatCompletionTarget,
        history: Vec<StoredChatMessage>,
    ) -> Self {
        Self {
            stored,
            client,
            target,
            history,
            listeners: Arc::new(ListenerRegistry::new()),
            permission: Mutex::new(PermissionMode::Auto),
            approval_handler: Mutex::new(None),
            question_handler: Mutex::new(None),
        }
    }

    fn emit(&self, kind: PromptEventKind) {
        self.listeners.emit(PromptEvent {
            session_id: self.stored.id.clone(),
            agent_id: "main".to_owned(),
            kind,
        });
    }

    async fn stream_completion(&self, prompt: String) -> Result<(), PromptSessionError> {
        // MIGRATION-TODO:
        // Original: the agent-core Session builds system instructions, durable
        // history, tools, hooks, retry policy, and a multi-step tool loop.
        // Temporary behavior: durable text history is sent directly to the
        // selected chat-completions model and only text/thinking deltas are
        // decoded. A 401 is surfaced without the original forced-refresh retry.
        // Completion condition: compose the migrated agent/session engine here.
        self.stored.append_user_prompt(&prompt).await?;
        let mut messages = self
            .history
            .iter()
            .map(|message| {
                json!({
                    "role": message.role,
                    "content": message.content,
                })
            })
            .collect::<Vec<_>>();
        messages.push(json!({ "role": "user", "content": prompt }));
        let request = self
            .client
            .post(&self.target.endpoint)
            .bearer_auth(&self.target.api_key)
            .headers(self.target.headers.clone())
            .json(&json!({
                "model": self.target.model,
                "messages": messages,
                "stream": true,
                "stream_options": { "include_usage": true }
            }));
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(http_error(status, response).await.into());
        }

        self.emit(PromptEventKind::TurnStarted { turn_id: 1 });
        self.emit(PromptEventKind::TurnStepStarted { turn_id: 1 });
        let mut bytes = Vec::new();
        let mut completion = CompletionAccumulator::default();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            bytes.extend_from_slice(&chunk?);
            while let Some(index) = bytes.iter().position(|byte| *byte == b'\n') {
                let line = bytes.drain(..=index).collect::<Vec<_>>();
                self.handle_sse_line(&line[..line.len().saturating_sub(1)], &mut completion)?;
            }
        }
        if !bytes.is_empty() {
            self.handle_sse_line(&bytes, &mut completion)?;
        }
        self.stored
            .append_assistant_message(&completion.thinking, &completion.content)
            .await?;
        self.emit(PromptEventKind::TurnEnded {
            turn_id: 1,
            reason: super::run_prompt::TurnEndReason::Completed,
            error: None,
        });
        Ok(())
    }

    fn handle_sse_line(
        &self,
        line: &[u8],
        completion: &mut CompletionAccumulator,
    ) -> Result<(), PromptSessionError> {
        let line = std::str::from_utf8(line)?.trim_end_matches('\r');
        let Some(data) = line.strip_prefix("data:") else {
            return Ok(());
        };
        let data = data.trim_start();
        if data.is_empty() || data == "[DONE]" {
            return Ok(());
        }
        let value: Value = serde_json::from_str(data)?;
        if let Some(message) = value
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
        {
            return Err(io::Error::other(message.to_owned()).into());
        }
        let Some(delta) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("delta"))
            .and_then(Value::as_object)
        else {
            return Ok(());
        };
        if let Some(thinking) = reasoning_delta(delta) {
            completion.thinking.push_str(thinking);
            self.emit(PromptEventKind::ThinkingDelta {
                turn_id: 1,
                delta: thinking.to_owned(),
            });
        }
        if let Some(content) = delta.get("content").and_then(Value::as_str) {
            completion.content.push_str(content);
            self.emit(PromptEventKind::AssistantDelta {
                turn_id: 1,
                delta: content.to_owned(),
            });
        }
        Ok(())
    }
}

#[derive(Default)]
struct CompletionAccumulator {
    thinking: String,
    content: String,
}

fn reasoning_delta(delta: &Map<String, Value>) -> Option<&str> {
    ["reasoning_content", "reasoning_details", "reasoning"]
        .into_iter()
        .find_map(|key| delta.get(key).and_then(Value::as_str))
}

async fn http_error(status: StatusCode, response: reqwest::Response) -> io::Error {
    let body = response.text().await.unwrap_or_default();
    let message = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.trim().to_owned());
    let suffix = if message.is_empty() {
        String::new()
    } else {
        format!(": {message}")
    };
    io::Error::other(format!("model request failed with HTTP {status}{suffix}"))
}

#[async_trait]
impl PromptSession for HttpPromptSession {
    fn id(&self) -> &str {
        &self.stored.id
    }

    fn work_dir(&self) -> &str {
        &self.stored.work_dir
    }

    async fn get_status(&self) -> Result<SessionStatus, PromptSessionError> {
        Ok(SessionStatus {
            model: Some(self.target.alias.clone()),
            thinking_effort: "off".to_owned(),
            permission: *self
                .permission
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            plan_mode: false,
            swarm_mode: None,
            context_tokens: 0,
            max_context_tokens: 0,
            context_usage: 0.0,
            usage: None,
        })
    }

    async fn set_model(&self, model: &str) -> Result<(), PromptSessionError> {
        if model == self.target.alias {
            Ok(())
        } else {
            Err(
                io::Error::other("model switching requires the durable Rust session runtime")
                    .into(),
            )
        }
    }

    async fn set_permission(&self, mode: PermissionMode) -> Result<(), PromptSessionError> {
        *self
            .permission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = mode;
        Ok(())
    }

    fn set_approval_handler(&self, handler: Option<ApprovalHandler>) {
        *self
            .approval_handler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = handler;
    }

    fn set_question_handler(&self, handler: Option<QuestionHandler>) {
        *self
            .question_handler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = handler;
    }

    fn on_event(&self, listener: EventListener) -> Unsubscribe {
        let id = self.listeners.next_id.fetch_add(1, Ordering::Relaxed);
        self.listeners
            .listeners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, listener);
        let listeners: Weak<ListenerRegistry> = Arc::downgrade(&self.listeners);
        Box::new(move || {
            if let Some(listeners) = listeners.upgrade() {
                listeners
                    .listeners
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&id);
            }
        })
    }

    async fn prompt(&self, input: PromptInput) -> Result<(), PromptSessionError> {
        let prompt = match input {
            PromptInput::Text(prompt) => prompt,
            PromptInput::Parts(parts) => prompt_parts_to_text(parts)?,
        };
        self.stream_completion(prompt).await
    }

    async fn wait_for_background_tasks_on_print(&self) -> Result<(), PromptSessionError> {
        Ok(())
    }

    async fn handle_print_main_turn_completed(
        &self,
    ) -> Result<PrintTurnAction, PromptSessionError> {
        Ok(PrintTurnAction::Finish)
    }

    async fn create_goal(&self, _: CreateGoalInput) -> Result<GoalSnapshot, PromptSessionError> {
        Err(io::Error::other("goals require the durable Rust session runtime").into())
    }

    async fn get_goal(&self) -> Result<Option<GoalSnapshot>, PromptSessionError> {
        Ok(None)
    }

    async fn get_cron_tasks(&self) -> Result<Vec<CronTaskSnapshot>, PromptSessionError> {
        Ok(Vec::new())
    }
}

fn prompt_parts_to_text(parts: Vec<PromptPart>) -> Result<String, PromptSessionError> {
    let mut text = String::new();
    for part in parts {
        match part {
            PromptPart::Text { text: part } => text.push_str(&part),
            PromptPart::ImageUrl { .. } | PromptPart::VideoUrl { .. } => {
                return Err(io::Error::other(
                    "multimodal prompt parts are not yet supported by the Rust prompt runtime",
                )
                .into());
            }
        }
    }
    Ok(text)
}

pub struct ProcessPromptOutput {
    stderr: bool,
}

impl ProcessPromptOutput {
    pub const fn stdout() -> Self {
        Self { stderr: false }
    }

    pub const fn stderr() -> Self {
        Self { stderr: true }
    }
}

impl PromptOutput for ProcessPromptOutput {
    fn write(&mut self, chunk: &str) -> bool {
        if self.stderr {
            let mut output = io::stderr().lock();
            output.write_all(chunk.as_bytes()).is_ok() && output.flush().is_ok()
        } else {
            let mut output = io::stdout().lock();
            output.write_all(chunk.as_bytes()).is_ok() && output.flush().is_ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use base64::Engine;
    use uuid::Uuid;

    use super::*;

    #[derive(Default)]
    struct Capture(String);

    impl PromptOutput for Capture {
        fn write(&mut self, chunk: &str) -> bool {
            self.0.push_str(chunk);
            true
        }
    }

    fn fake_chat_server(
        status: &'static str,
        response_body: &'static str,
    ) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake model server");
        let address = listener.local_addr().expect("fake server address");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept model request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let count = stream.read(&mut buffer).expect("read model request");
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
                let header_end = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|index| index + 4);
                let Some(header_end) = header_end else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if request.len() >= header_end + content_length {
                    break;
                }
            }
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            )
            .expect("write model response");
            String::from_utf8(request).expect("utf8 request")
        });
        (format!("http://{address}/v1"), handle)
    }

    fn temp_data_dir() -> PathBuf {
        std::env::temp_dir().join(format!("kimi-rust-prompt-{}", Uuid::new_v4()))
    }

    fn provider(provider_type: &str) -> ProviderDefinition {
        ProviderDefinition {
            provider_type: provider_type.to_owned(),
            base_url: None,
            api_key: None,
            oauth: None,
            source: None,
            additional_fields: Map::new(),
        }
    }

    #[tokio::test]
    async fn fresh_prompt_reaches_chat_completions_and_streams_text() {
        let response = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"想\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"你\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"好\"}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let (base_url, server) = fake_chat_server("200 OK", response);
        let data_dir = temp_data_dir();
        fs::create_dir_all(&data_dir).expect("create data dir");
        fs::write(
            data_dir.join("config.toml"),
            format!(
                "default_model = \"local/test\"\n\n[providers.local]\ntype = \"openai\"\nbase_url = \"{base_url}\"\napi_key = \"secret\"\n\n[models.\"local/test\"]\nprovider = \"local\"\nmodel = \"test-model\"\n"
            ),
        )
        .expect("write config");
        let runtime = SystemPromptRuntime::with_client(
            &data_dir,
            HashMap::new(),
            Client::builder().build().expect("client"),
        );
        let options = CliOptions {
            prompt: Some("打个招呼".to_owned()),
            ..CliOptions::default()
        };
        let mut stdout = Capture::default();
        let mut stderr = Capture::default();

        runtime
            .run(&options, &mut stdout, &mut stderr)
            .await
            .expect("run prompt");

        assert_eq!(stdout.0, "• 你好\n\n");
        assert!(
            stderr
                .0
                .starts_with("• 想\n\nTo resume this session: kimi -r session_")
        );
        let request = server.join().expect("model server");
        assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
        assert!(request.contains("authorization: Bearer secret"));
        assert!(request.contains("\"model\":\"test-model\""));
        assert!(request.contains("\"content\":\"打个招呼\""));
        fs::remove_dir_all(data_dir).expect("remove data dir");
    }

    #[tokio::test]
    async fn resumed_prompt_sends_durable_history_and_reuses_session_id() {
        let response = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"new answer\"}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let (base_url, server) = fake_chat_server("200 OK", response);
        let data_dir = temp_data_dir();
        fs::create_dir_all(&data_dir).expect("create data dir");
        fs::write(
            data_dir.join("config.toml"),
            format!(
                "default_model = \"local/test\"\n\n[providers.local]\ntype = \"openai\"\nbase_url = \"{base_url}\"\napi_key = \"secret\"\n\n[models.\"local/test\"]\nprovider = \"local\"\nmodel = \"test-model\"\n"
            ),
        )
        .expect("write config");
        let work_dir = std::env::current_dir().expect("cwd");
        let store = PromptSessionStore::new(&data_dir);
        let stored = store
            .create(work_dir.to_str().expect("utf8 cwd"), "local/test")
            .await
            .expect("create stored session");
        stored
            .append_user_prompt("old question")
            .await
            .expect("old prompt");
        stored
            .append_assistant_message("", "old answer")
            .await
            .expect("old answer");
        let session_id = stored.id.clone();
        let runtime = SystemPromptRuntime::with_client(
            &data_dir,
            HashMap::new(),
            Client::builder().build().expect("client"),
        );
        let mut stdout = Capture::default();
        let mut stderr = Capture::default();

        runtime
            .run(
                &CliOptions {
                    session: Some(session_id.clone()),
                    prompt: Some("new question".to_owned()),
                    ..CliOptions::default()
                },
                &mut stdout,
                &mut stderr,
            )
            .await
            .expect("resume prompt");

        assert_eq!(stdout.0, "• new answer\n\n");
        assert!(stderr.0.contains(&format!("kimi -r {session_id}")));
        let request = server.join().expect("model server");
        let old_question = request.find("old question").expect("old question sent");
        let old_answer = request.find("old answer").expect("old answer sent");
        let new_question = request.find("new question").expect("new question sent");
        assert!(old_question < old_answer && old_answer < new_question);
        let history = stored.load_history().await.expect("restored history");
        assert_eq!(history.last().unwrap().content, "new answer");
        fs::remove_dir_all(data_dir).expect("remove data dir");
    }

    #[tokio::test]
    async fn continue_without_session_warns_then_creates_a_resumable_session() {
        let response = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"answer\"}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let (base_url, server) = fake_chat_server("200 OK", response);
        let data_dir = temp_data_dir();
        fs::create_dir_all(&data_dir).expect("create data dir");
        fs::write(
            data_dir.join("config.toml"),
            format!(
                "default_model = \"local/test\"\n\n[providers.local]\ntype = \"openai\"\nbase_url = \"{base_url}\"\napi_key = \"secret\"\n\n[models.\"local/test\"]\nprovider = \"local\"\nmodel = \"test-model\"\n"
            ),
        )
        .expect("write config");
        let runtime = SystemPromptRuntime::with_client(
            &data_dir,
            HashMap::new(),
            Client::builder().build().expect("client"),
        );
        let mut stderr = Capture::default();

        runtime
            .run(
                &CliOptions {
                    continue_previous: true,
                    prompt: Some("question".to_owned()),
                    ..CliOptions::default()
                },
                &mut Capture::default(),
                &mut stderr,
            )
            .await
            .expect("continue prompt");

        assert!(stderr.0.starts_with("No sessions to continue under \""));
        assert!(stderr.0.contains("starting a fresh session.\n"));
        assert!(
            stderr
                .0
                .contains("To resume this session: kimi -r session_")
        );
        server.join().expect("model server");
        fs::remove_dir_all(data_dir).expect("remove data dir");
    }

    #[tokio::test]
    async fn model_http_error_preserves_status_and_provider_message() {
        let (base_url, server) = fake_chat_server(
            "401 Unauthorized",
            r#"{"error":{"message":"expired credential"}}"#,
        );
        let data_dir = temp_data_dir();
        fs::create_dir_all(&data_dir).expect("create data dir");
        fs::write(
            data_dir.join("config.toml"),
            format!(
                "default_model = \"local/test\"\n\n[providers.local]\ntype = \"kimi\"\nbase_url = \"{base_url}\"\napi_key = \"expired\"\n\n[models.\"local/test\"]\nprovider = \"local\"\nmodel = \"test-model\"\n"
            ),
        )
        .expect("write config");
        let runtime = SystemPromptRuntime::with_client(
            &data_dir,
            HashMap::new(),
            Client::builder().build().expect("client"),
        );
        let error = runtime
            .run(
                &CliOptions {
                    prompt: Some("hello".to_owned()),
                    ..CliOptions::default()
                },
                &mut Capture::default(),
                &mut Capture::default(),
            )
            .await
            .expect_err("model error");

        assert!(error.to_string().contains("HTTP 401 Unauthorized"));
        assert!(error.to_string().contains("expired credential"));
        server.join().expect("model server");
        fs::remove_dir_all(data_dir).expect("remove data dir");
    }

    #[tokio::test]
    async fn unsupported_wire_type_is_reported_before_network_io() {
        let data_dir = temp_data_dir();
        fs::create_dir_all(&data_dir).expect("create data dir");
        fs::write(
            data_dir.join("config.toml"),
            "default_model = \"a/m\"\n\n[providers.a]\ntype = \"anthropic\"\napi_key = \"secret\"\n\n[models.\"a/m\"]\nprovider = \"a\"\nmodel = \"claude\"\n",
        )
        .expect("write config");
        let runtime = SystemPromptRuntime::with_client(
            &data_dir,
            HashMap::new(),
            Client::builder().build().expect("client"),
        );
        let options = CliOptions {
            prompt: Some("hello".to_owned()),
            ..CliOptions::default()
        };
        let error = runtime
            .run(&options, &mut Capture::default(), &mut Capture::default())
            .await
            .expect_err("unsupported provider");

        assert!(error.to_string().contains("anthropic"));
        assert!(
            error
                .to_string()
                .contains("currently supported: openai, kimi")
        );
        fs::remove_dir_all(data_dir).expect("remove data dir");
    }

    #[test]
    fn provider_env_subtable_is_the_credential_fallback() {
        let mut provider = provider("openai");
        provider.additional_fields.insert(
            "env".to_owned(),
            json!({
                "OPENAI_API_KEY": "from-config",
                "OPENAI_BASE_URL": "https://config.example/v1"
            }),
        );
        let shell = HashMap::from([
            ("OPENAI_API_KEY".to_owned(), "from-shell".to_owned()),
            (
                "OPENAI_BASE_URL".to_owned(),
                "https://shell.example/v1".to_owned(),
            ),
        ]);

        assert_eq!(provider_config_api_key(&provider), Some("from-config"));
        assert_eq!(
            resolved_base_url("custom", &provider, &shell),
            "https://config.example/v1"
        );
    }

    #[test]
    fn managed_base_url_environment_override_takes_precedence_over_config() {
        let mut provider = provider("kimi");
        provider.base_url = Some("https://configured.example/v1".to_owned());
        let environment = HashMap::from([(
            "KIMI_CODE_BASE_URL".to_owned(),
            "https://environment.example/v1".to_owned(),
        )]);

        assert_eq!(
            resolved_base_url(KIMI_CODE_PROVIDER_NAME, &provider, &environment),
            "https://environment.example/v1"
        );
    }

    #[test]
    fn kimi_headers_include_identity_and_preserve_provider_override_order() {
        let data_dir = temp_data_dir();
        let mut provider = provider("kimi");
        provider.additional_fields.insert(
            "customHeaders".to_owned(),
            json!({ "User-Agent": "provider-agent", "X-Test": "provider" }),
        );
        let environment = HashMap::from([(
            "KIMI_CODE_CUSTOM_HEADERS".to_owned(),
            "X-Test: environment\nX-Environment: yes".to_owned(),
        )]);

        let headers = configured_headers(
            &provider,
            &environment,
            Some(KimiIdentityOptions {
                home_dir: data_dir.clone(),
                host: KimiHostIdentity {
                    user_agent_product: "kimi-code-cli".to_owned(),
                    version: "1.2.3".to_owned(),
                    user_agent_suffix: None,
                },
            }),
        )
        .expect("configured headers");

        assert_eq!(headers["user-agent"], "provider-agent");
        assert_eq!(headers["x-test"], "provider");
        assert_eq!(headers["x-environment"], "yes");
        assert_eq!(headers["x-msh-platform"], "kimi_code_cli");
        assert!(headers.contains_key("x-msh-device-id"));
        fs::remove_dir_all(data_dir).expect("remove identity data dir");
    }

    #[test]
    fn text_only_prompt_parts_are_flattened_and_multimodal_parts_are_explicitly_pending() {
        assert_eq!(
            prompt_parts_to_text(vec![
                PromptPart::Text {
                    text: "one".to_owned()
                },
                PromptPart::Text {
                    text: "two".to_owned()
                },
            ])
            .expect("text parts"),
            "onetwo"
        );
        let image = base64::engine::general_purpose::STANDARD.encode("image");
        let error = prompt_parts_to_text(vec![PromptPart::ImageUrl {
            image_url: crate::sdk::types::MediaUrl {
                url: format!("data:image/png;base64,{image}"),
                id: None,
            },
        }])
        .expect_err("multimodal pending");
        assert!(error.to_string().contains("multimodal"));
    }
}
