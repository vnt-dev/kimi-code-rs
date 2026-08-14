//! Config-aware OAuth service.
//!
//! Original: `packages/agent-core-v2/src/app/auth/authService.ts`,
//! `OAuthService`.

use parking_lot::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::{collections::HashMap, error::Error, num::NonZeroU64, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use kimi_code_oauth::{
    BearerTokenProvider, CredentialKind, DeviceAuthorization, DeviceCodeObserver,
    KIMI_CODE_PROVIDER_NAME, KimiOAuthLoginOptions, KimiOAuthTokenRef, LoginAbortSignal,
    ManagedKimiCodeApplyOptions, OAuthError, OAuthErrorKind, OAuthManagerError,
    apply_managed_kimi_code_config, clear_managed_kimi_code_config, fetch_managed_kimi_code_models,
};
use serde_json::{Map, Value};
use tokio::sync::{Mutex as AsyncMutex, oneshot};
use url::Url;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        log::{LOG_SERVICE_ID, LogPayload, LogServiceHandle},
    },
    app::{
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle, ConfigTarget},
        event::{EVENT_SERVICE_ID, EventServiceHandle, global_event::GlobalDomainEvent},
        telemetry::{TELEMETRY_SERVICE_ID, TelemetryServiceHandle},
    },
    kosong::{
        model::contract::{DEFAULT_MODEL_SECTION, MODELS_SECTION},
        provider::{
            OAuthRef, OAuthStorage, PROVIDER_SERVICE_ID, PROVIDERS_SECTION, ProviderConfig,
            ProviderServiceHandle, ProviderType,
        },
    },
};

use super::{
    AlwaysTrue, AuthOperationError, AuthStatus, NonEmptyString, OAUTH_SERVICE_ID, OAUTH_TOOLKIT_ID,
    OAuthFlowSnapshot, OAuthFlowStart, OAuthFlowStartAuthenticated, OAuthFlowStartPending,
    OAuthFlowStatus, OAuthLoginCancelResponse, OAuthLogoutResponse, OAuthServiceContract,
    OAuthServiceHandle, OAuthToolkitHandle, ProviderRefreshChange, ProviderRefreshFailure,
    RefreshOAuthProviderModelsResponse,
};

const TERMINAL_RETENTION: Duration = Duration::from_secs(5 * 60);
const DEFAULT_DEVICE_EXPIRES_IN_SEC: u64 = 15 * 60;
const THINKING_SECTION: &str = "thinking";
const SERVICES_SECTION: &str = "services";

struct FlowState {
    flow_id: String,
    provider: String,
    cancelled: Arc<AtomicBool>,
    device: Option<DeviceAuthorization>,
    status: OAuthFlowStatus,
    expires_at: DateTime<Utc>,
    error_message: Option<String>,
    resolved_at: Option<DateTime<Utc>>,
}

struct DeviceObserver {
    sender: Mutex<Option<oneshot::Sender<DeviceAuthorization>>>,
}

#[async_trait]
impl DeviceCodeObserver for DeviceObserver {
    async fn on_device_code(
        &self,
        authorization: &DeviceAuthorization,
    ) -> Result<(), OAuthManagerError> {
        if let Some(sender) = self.sender.lock().take() {
            let _ = sender.send(authorization.clone());
        }
        Ok(())
    }
}

struct AbortFlag(Arc<AtomicBool>);

impl LoginAbortSignal for AbortFlag {
    fn is_aborted(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
struct OAuthWorker {
    toolkit: OAuthToolkitHandle,
    providers: ProviderServiceHandle,
    config: ConfigServiceHandle,
    telemetry: TelemetryServiceHandle,
    log: LogServiceHandle,
    events: EventServiceHandle,
}

pub struct OAuthService {
    worker: OAuthWorker,
    flows: Arc<Mutex<HashMap<String, Arc<Mutex<FlowState>>>>>,
    refresh_gate: AsyncMutex<()>,
}

impl OAuthService {
    pub fn new(
        toolkit: OAuthToolkitHandle,
        providers: ProviderServiceHandle,
        config: ConfigServiceHandle,
        telemetry: TelemetryServiceHandle,
        log: LogServiceHandle,
        events: EventServiceHandle,
    ) -> Self {
        Self {
            worker: OAuthWorker {
                toolkit,
                providers,
                config,
                telemetry,
                log,
                events,
            },
            flows: Arc::new(Mutex::new(HashMap::new())),
            refresh_gate: AsyncMutex::new(()),
        }
    }

    fn provider_name(provider: Option<&str>) -> String {
        provider.unwrap_or(KIMI_CODE_PROVIDER_NAME).to_owned()
    }

    fn oauth_ref(reference: Option<&OAuthRef>) -> Option<KimiOAuthTokenRef> {
        reference.map(|reference| KimiOAuthTokenRef {
            key: Some(reference.key.clone()),
            oauth_host: reference.oauth_host.clone(),
        })
    }

    fn runtime_oauth_ref(&self, provider: &str, requested: Option<&OAuthRef>) -> Option<OAuthRef> {
        requested.cloned().or_else(|| {
            self.worker
                .providers
                .get(provider)
                .and_then(|value| value.oauth)
        })
    }

    fn abort_existing(&self, provider: &str) {
        let existing = self.flows.lock().get(provider).cloned();
        if let Some(existing) = existing {
            let mut state = existing.lock();
            if state.status == OAuthFlowStatus::Pending {
                state.cancelled.store(true, Ordering::Release);
                Self::set_terminal(&mut state, OAuthFlowStatus::Cancelled, None);
            }
        }
    }

    fn set_terminal(state: &mut FlowState, status: OAuthFlowStatus, error_message: Option<String>) {
        state.status = status;
        state.error_message = error_message;
        state.resolved_at = Some(Utc::now());
    }

    fn schedule_cleanup(
        flows: Arc<Mutex<HashMap<String, Arc<Mutex<FlowState>>>>>,
        provider: String,
        state: Arc<Mutex<FlowState>>,
    ) {
        tokio::spawn(async move {
            tokio::time::sleep(TERMINAL_RETENTION).await;
            let mut flows = flows.lock();
            if flows
                .get(&provider)
                .is_some_and(|candidate| Arc::ptr_eq(candidate, &state))
            {
                flows.remove(&provider);
            }
        });
    }

    fn flow_start(
        state: &FlowState,
        device: &DeviceAuthorization,
    ) -> Result<OAuthFlowStart, AuthOperationError> {
        let expires_in =
            positive_seconds(device.expires_in.unwrap_or(DEFAULT_DEVICE_EXPIRES_IN_SEC));
        let interval = positive_seconds(device.interval);
        Ok(OAuthFlowStart::Pending(Box::new(OAuthFlowStartPending {
            flow_id: non_empty(&state.flow_id)?,
            provider: non_empty(&state.provider)?,
            verification_uri: parse_url(&device.verification_uri)?,
            verification_uri_complete: parse_url(&device.verification_uri_complete)?,
            user_code: non_empty(&device.user_code)?,
            expires_in,
            interval,
            expires_at: state.expires_at,
        })))
    }

    fn snapshot(state: &FlowState) -> Option<OAuthFlowSnapshot> {
        let device = state.device.as_ref()?;
        Some(OAuthFlowSnapshot {
            flow_id: NonEmptyString::new(state.flow_id.clone()).ok()?,
            provider: NonEmptyString::new(state.provider.clone()).ok()?,
            status: state.status,
            verification_uri: Url::parse(&device.verification_uri).ok()?,
            verification_uri_complete: Url::parse(&device.verification_uri_complete).ok()?,
            user_code: NonEmptyString::new(device.user_code.clone()).ok()?,
            expires_in: positive_seconds(
                device.expires_in.unwrap_or(DEFAULT_DEVICE_EXPIRES_IN_SEC),
            ),
            expires_at: state.expires_at,
            interval: positive_seconds(device.interval),
            resolved_at: state.resolved_at,
            error_message: state.error_message.clone(),
        })
    }

    async fn refresh_models(
        &self,
    ) -> Result<RefreshOAuthProviderModelsResponse, AuthOperationError> {
        let _guard = self.refresh_gate.lock().await;
        self.worker.config.reload().await.map_err(operation_error)?;
        let Some(provider) = self.worker.providers.get(KIMI_CODE_PROVIDER_NAME) else {
            return Ok(RefreshOAuthProviderModelsResponse {
                changed: Vec::new(),
                unchanged: Vec::new(),
                failed: Vec::new(),
            });
        };
        let oauth_ref = Self::oauth_ref(provider.oauth.as_ref());
        let token = self
            .worker
            .toolkit
            .get_cached_access_token(Some(KIMI_CODE_PROVIDER_NAME), oauth_ref.as_ref())
            .await?
            .ok_or_else(|| AuthOperationError::new("OAuth token provider is not configured."))?;
        let models = match fetch_managed_kimi_code_models(
            &token,
            provider.base_url.as_deref(),
            None,
            CredentialKind::OAuth,
        )
        .await
        {
            Ok(models) => models,
            Err(error) => {
                return Ok(RefreshOAuthProviderModelsResponse {
                    changed: Vec::new(),
                    unchanged: Vec::new(),
                    failed: vec![ProviderRefreshFailure {
                        provider: non_empty(KIMI_CODE_PROVIDER_NAME)?,
                        reason: non_empty(error.to_string())?,
                    }],
                });
            }
        };
        if models.is_empty() {
            return Ok(RefreshOAuthProviderModelsResponse {
                changed: Vec::new(),
                unchanged: vec![non_empty(KIMI_CODE_PROVIDER_NAME)?],
                failed: Vec::new(),
            });
        }

        let mut config = self.user_config_shape();
        let before = config.get("models").cloned();
        apply_managed_kimi_code_config(
            &mut config,
            ManagedKimiCodeApplyOptions {
                models: &models,
                base_url: provider.base_url.as_deref(),
                oauth_key: provider
                    .oauth
                    .as_ref()
                    .map(|reference| reference.key.as_str()),
                oauth_host: provider
                    .oauth
                    .as_ref()
                    .and_then(|reference| reference.oauth_host.as_deref()),
                preserve_default_model: true,
            },
        )
        .map_err(operation_error)?;
        if before == config.get("models").cloned() {
            return Ok(RefreshOAuthProviderModelsResponse {
                changed: Vec::new(),
                unchanged: vec![non_empty(KIMI_CODE_PROVIDER_NAME)?],
                failed: Vec::new(),
            });
        }
        self.write_managed_config(&config).await?;
        let result = RefreshOAuthProviderModelsResponse {
            changed: vec![ProviderRefreshChange {
                provider_id: non_empty(KIMI_CODE_PROVIDER_NAME)?,
                provider_name: non_empty("Kimi Code")?,
                added: models.len() as u64,
                removed: 0,
            }],
            unchanged: Vec::new(),
            failed: Vec::new(),
        };
        self.worker.events.publish(GlobalDomainEvent {
            event_type: "event.model_catalog.changed".into(),
            payload: serde_json::to_value(&result).map_err(operation_error)?,
        });
        Ok(result)
    }

    fn user_config_shape(&self) -> Map<String, Value> {
        let mut result = Map::new();
        for section in [
            PROVIDERS_SECTION,
            MODELS_SECTION,
            SERVICES_SECTION,
            DEFAULT_MODEL_SECTION,
            THINKING_SECTION,
        ] {
            if let Some(value) = self.worker.config.inspect(section).user_value {
                result.insert(section.into(), value);
            }
        }
        result
    }

    async fn write_managed_config(
        &self,
        config: &Map<String, Value>,
    ) -> Result<(), AuthOperationError> {
        for section in [PROVIDERS_SECTION, MODELS_SECTION, SERVICES_SECTION] {
            self.worker
                .config
                .replace(section, config.get(section).cloned(), ConfigTarget::User)
                .await
                .map_err(operation_error)?;
        }
        for section in [DEFAULT_MODEL_SECTION, THINKING_SECTION] {
            self.worker
                .config
                .set(section, config.get(section).cloned(), ConfigTarget::User)
                .await
                .map_err(operation_error)?;
        }
        Ok(())
    }

    async fn deprovision(&self, provider: &str) -> Result<(), AuthOperationError> {
        if provider != KIMI_CODE_PROVIDER_NAME {
            self.worker
                .providers
                .delete(provider)
                .await
                .map_err(operation_error)?;
            return Ok(());
        }
        let mut config = self.user_config_shape();
        let cleanup = clear_managed_kimi_code_config(&mut config);
        if cleanup.removed_provider
            || !cleanup.removed_models.is_empty()
            || cleanup.default_model_cleared
            || !cleanup.removed_services.is_empty()
        {
            self.write_managed_config(&config).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl OAuthServiceContract for OAuthService {
    async fn start_login(
        &self,
        provider: Option<&str>,
    ) -> Result<OAuthFlowStart, AuthOperationError> {
        let provider = Self::provider_name(provider);
        self.worker.log.0.info(
            "oauth startLogin: enter",
            Some(LogPayload::Value(serde_json::json!({"provider": provider}))),
        );
        self.worker.telemetry.track(
            "oauth_login_started",
            Some(&crate::app::telemetry::TelemetryProperties::from([(
                "provider".into(),
                Some(Value::String(provider.clone())),
            )])),
        );
        self.abort_existing(&provider);
        let configured = self.worker.providers.get(&provider);
        let oauth_ref = configured
            .as_ref()
            .and_then(|provider| Self::oauth_ref(provider.oauth.as_ref()));
        let base_url = configured
            .as_ref()
            .and_then(|provider| provider.base_url.clone());
        let cancelled = Arc::new(AtomicBool::new(false));
        let flow_id = format!("oauth_{}", uuid::Uuid::new_v4());
        let state = Arc::new(Mutex::new(FlowState {
            flow_id: flow_id.clone(),
            provider: provider.clone(),
            cancelled: Arc::clone(&cancelled),
            device: None,
            status: OAuthFlowStatus::Pending,
            expires_at: Utc::now()
                + chrono::Duration::seconds(DEFAULT_DEVICE_EXPIRES_IN_SEC as i64),
            error_message: None,
            resolved_at: None,
        }));
        self.flows
            .lock()
            .insert(provider.clone(), Arc::clone(&state));

        let (device_tx, device_rx) = oneshot::channel();
        let observer = DeviceObserver {
            sender: Mutex::new(Some(device_tx)),
        };
        let abort = AbortFlag(cancelled);
        let toolkit = self.worker.toolkit.clone();
        let providers = self.worker.providers.clone();
        let state_for_task = Arc::clone(&state);
        let flows_for_task = Arc::clone(&self.flows);
        let provider_for_task = provider.clone();
        let oauth_for_task = oauth_ref.clone();
        let base_for_task = base_url.clone();
        let (done_tx, done_rx) = oneshot::channel();
        tokio::spawn(async move {
            let result = toolkit
                .login(
                    Some(&provider_for_task),
                    KimiOAuthLoginOptions {
                        on_device_code: Some(&observer),
                        signal: Some(&abort),
                        provision_config: Some(false),
                        base_url: base_for_task.as_deref(),
                        oauth_ref: oauth_for_task.as_ref(),
                        oauth_host: oauth_for_task
                            .as_ref()
                            .and_then(|reference| reference.oauth_host.as_deref()),
                    },
                )
                .await;
            match result {
                Ok(_) => {
                    let reference = oauth_for_task.as_ref().map(|reference| {
                        OAuthRef::new(
                            OAuthStorage::File,
                            reference.key.clone().unwrap_or_default(),
                            reference.oauth_host.clone(),
                        )
                    });
                    if let Some(Ok(reference)) = reference {
                        let _ = providers
                            .set(
                                &provider_for_task,
                                ProviderConfig {
                                    provider_type: Some(ProviderType::from("kimi")),
                                    base_url: base_for_task,
                                    api_key: Some(String::new()),
                                    oauth: Some(reference),
                                    ..ProviderConfig::default()
                                },
                            )
                            .await;
                    }
                    Self::set_terminal(
                        &mut state_for_task.lock(),
                        OAuthFlowStatus::Authenticated,
                        None,
                    );
                    Self::schedule_cleanup(
                        flows_for_task,
                        provider_for_task.clone(),
                        Arc::clone(&state_for_task),
                    );
                    let _ = done_tx.send(Ok(()));
                }
                Err(error) => {
                    let message = error.to_string();
                    let status = if state_for_task.lock().cancelled.load(Ordering::Acquire) {
                        OAuthFlowStatus::Cancelled
                    } else if is_oauth_expired(&error) {
                        OAuthFlowStatus::Expired
                    } else {
                        OAuthFlowStatus::Denied
                    };
                    Self::set_terminal(&mut state_for_task.lock(), status, Some(message.clone()));
                    Self::schedule_cleanup(
                        flows_for_task,
                        provider_for_task.clone(),
                        Arc::clone(&state_for_task),
                    );
                    let _ = done_tx.send(Err(AuthOperationError::new(message)));
                }
            }
        });

        tokio::select! {
            device = device_rx => {
                let device = device.map_err(|_| AuthOperationError::new("OAuth login ended before a device code was issued"))?;
                let mut state = state.lock();
                state.expires_at = Utc::now() + chrono::Duration::seconds(
                    positive_seconds(device.expires_in.unwrap_or(DEFAULT_DEVICE_EXPIRES_IN_SEC)).get() as i64
                );
                state.device = Some(device.clone());
                Self::flow_start(&state, &device)
            }
            done = done_rx => {
                done.map_err(|_| AuthOperationError::new("OAuth login task ended unexpectedly"))??;
                Ok(OAuthFlowStart::Authenticated(OAuthFlowStartAuthenticated {
                    flow_id: non_empty(flow_id)?,
                    provider: non_empty(provider)?,
                }))
            }
        }
    }

    fn get_flow(&self, provider: Option<&str>) -> Option<OAuthFlowSnapshot> {
        let provider = Self::provider_name(provider);
        let state = self.flows.lock().get(&provider).cloned()?;
        Self::snapshot(&state.lock())
    }

    async fn cancel_login(
        &self,
        provider: Option<&str>,
    ) -> Result<OAuthLoginCancelResponse, AuthOperationError> {
        let provider = Self::provider_name(provider);
        let Some(flow) = self.flows.lock().get(&provider).cloned() else {
            return Ok(OAuthLoginCancelResponse {
                cancelled: false,
                status: OAuthFlowStatus::Cancelled,
            });
        };
        let mut state = flow.lock();
        if state.status != OAuthFlowStatus::Pending {
            return Ok(OAuthLoginCancelResponse {
                cancelled: false,
                status: state.status,
            });
        }
        state.cancelled.store(true, Ordering::Release);
        Self::set_terminal(&mut state, OAuthFlowStatus::Cancelled, None);
        drop(state);
        Self::schedule_cleanup(Arc::clone(&self.flows), provider, flow);
        Ok(OAuthLoginCancelResponse {
            cancelled: true,
            status: OAuthFlowStatus::Cancelled,
        })
    }

    async fn logout(
        &self,
        provider: Option<&str>,
    ) -> Result<OAuthLogoutResponse, AuthOperationError> {
        let provider = Self::provider_name(provider);
        let reference = self.runtime_oauth_ref(&provider, None);
        let toolkit_reference = Self::oauth_ref(reference.as_ref());
        let result = self
            .worker
            .toolkit
            .logout(Some(&provider), toolkit_reference.as_ref())
            .await?;
        self.abort_existing(&provider);
        self.deprovision(&provider).await?;
        Ok(OAuthLogoutResponse {
            logged_out: AlwaysTrue,
            provider: non_empty(result.provider_name)?,
        })
    }

    async fn status(&self, provider: Option<&str>) -> Result<AuthStatus, AuthOperationError> {
        let provider = Self::provider_name(provider);
        let token = self.get_cached_access_token(&provider, None).await?;
        Ok(AuthStatus {
            logged_in: token.is_some(),
            provider: token.map(|_| provider),
        })
    }

    async fn refresh_oauth_provider_models(
        &self,
    ) -> Result<RefreshOAuthProviderModelsResponse, AuthOperationError> {
        self.refresh_models().await
    }

    fn resolve_token_provider(
        &self,
        provider: &str,
        oauth_ref: Option<&OAuthRef>,
    ) -> Option<BearerTokenProvider> {
        let reference = self.runtime_oauth_ref(provider, oauth_ref);
        let reference = Self::oauth_ref(reference.as_ref());
        self.worker
            .toolkit
            .token_provider(Some(provider), reference.as_ref())
            .ok()
    }

    async fn get_cached_access_token(
        &self,
        provider: &str,
        oauth_ref: Option<&OAuthRef>,
    ) -> Result<Option<String>, AuthOperationError> {
        let reference = self.runtime_oauth_ref(provider, oauth_ref);
        let reference = Self::oauth_ref(reference.as_ref());
        self.worker
            .toolkit
            .get_cached_access_token(Some(provider), reference.as_ref())
            .await
    }
}

fn non_empty(value: impl Into<String>) -> Result<NonEmptyString, AuthOperationError> {
    NonEmptyString::new(value).map_err(operation_error)
}

fn parse_url(value: &str) -> Result<Url, AuthOperationError> {
    Url::parse(value).map_err(operation_error)
}

fn positive_seconds(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value.max(1)).unwrap()
}

fn operation_error(error: impl std::fmt::Display) -> AuthOperationError {
    AuthOperationError::new(error.to_string())
}

/// Classifies the device-flow outcome by error type instead of message text.
/// The login error chain ends at `OAuthError`, which carries a typed
/// `OAuthErrorKind` for expired/timeout device codes.
fn is_oauth_expired(error: &(dyn Error + 'static)) -> bool {
    let mut current: Option<&(dyn Error + 'static)> = Some(error);
    while let Some(candidate) = current {
        if let Some(oauth) = candidate.downcast_ref::<OAuthError>() {
            return matches!(
                oauth.kind(),
                OAuthErrorKind::DeviceCodeExpired | OAuthErrorKind::DeviceCodeTimeout
            );
        }
        current = candidate.source();
    }
    false
}

pub fn register_oauth_service() {
    register_scoped_service(
        LifecycleScope::App,
        OAUTH_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let service = OAuthService::new(
                (*accessor.get(OAUTH_TOOLKIT_ID)?).clone(),
                (*accessor.get(PROVIDER_SERVICE_ID)?).clone(),
                (*accessor.get(CONFIG_SERVICE_ID)?).clone(),
                (*accessor.get(TELEMETRY_SERVICE_ID)?).clone(),
                (*accessor.get(LOG_SERVICE_ID)?).clone(),
                (*accessor.get(EVENT_SERVICE_ID)?).clone(),
            );
            Ok(OAuthServiceHandle(Arc::new(service)))
        }),
        InstantiationType::Eager,
        "auth",
    );
}
