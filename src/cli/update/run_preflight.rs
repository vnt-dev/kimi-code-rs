use std::{collections::HashMap, error::Error, fmt, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value, json};

use super::{
    preflight::{
        RolloutTelemetry, UpdatePlatform, decide_update_action, install_command_for,
        rollout_telemetry_for,
    },
    prompt::{InstallPromptChoiceValue, InstallPromptOptions},
    rollout::{
        PassiveUpdateDecision, decide_passive_update_target,
        is_rollout_bypassed_by_experimental_env,
    },
    types::{
        InstallSource, NPM_PACKAGE_NAME, UpdateCache, UpdateDecision, UpdateInstallState,
        UpdateManifest, UpdatePreflightResult, UpdateTarget, empty_update_cache,
        empty_update_install_state,
    },
};

#[derive(Debug)]
pub struct UpdatePreflightError(Box<dyn Error + Send + Sync>);

impl UpdatePreflightError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }

    pub fn message(message: impl Into<String>) -> Self {
        Self(Box::new(UpdatePreflightMessage(message.into())))
    }
}

impl fmt::Display for UpdatePreflightError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for UpdatePreflightError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[derive(Debug)]
struct UpdatePreflightMessage(String);

impl fmt::Display for UpdatePreflightMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for UpdatePreflightMessage {}

#[async_trait]
pub trait UpdatePreflightRuntime: Send + Sync + 'static {
    fn environment(&self) -> HashMap<String, String>;
    fn is_interactive(&self) -> bool;
    fn device_id(&self) -> String;
    fn platform(&self) -> UpdatePlatform;
    fn now(&self) -> DateTime<Utc>;

    async fn read_install_state(&self) -> Result<UpdateInstallState, UpdatePreflightError>;
    async fn show_pending_background_install_notice(
        &self,
        state: &UpdateInstallState,
        current_version: &str,
    ) -> Result<UpdateInstallState, UpdatePreflightError>;
    async fn read_update_cache(&self) -> Result<UpdateCache, UpdatePreflightError>;
    async fn refresh_update_cache(&self) -> Result<UpdateCache, UpdatePreflightError>;
    async fn detect_install_source(&self) -> Result<InstallSource, UpdatePreflightError>;
    async fn try_start_automatic_background_install(
        &self,
        install_state: &UpdateInstallState,
        current_version: &str,
        target: &UpdateTarget,
        source: InstallSource,
        rollout: &RolloutTelemetry,
    ) -> bool;
    async fn prompt_for_install_choice(
        &self,
        options: &InstallPromptOptions,
    ) -> Result<InstallPromptChoiceValue, UpdatePreflightError>;
    async fn install_update(
        &self,
        source: InstallSource,
        version: &str,
    ) -> Result<(), UpdatePreflightError>;

    fn append_rollout_decision_log(&self, entry: Map<String, Value>);
    fn track(&self, event: &str, properties: &Map<String, Value>);
    fn write_stdout(&self, text: &str);
    fn write_stderr(&self, text: &str);
}

// Original:
//   apps/kimi-code/src/cli/update/preflight.ts
//   runUpdatePreflight()
//
// Rust adaptation:
//   Fire-and-forget refreshes are tracked Tokio tasks. Dropping the runtime
//   still leaves already-spawned operating-system installers detached.
pub async fn run_update_preflight<R>(
    runtime: Arc<R>,
    current_version: &str,
) -> UpdatePreflightResult
where
    R: UpdatePreflightRuntime,
{
    let environment = runtime.environment();
    if super::preflight::is_auto_update_disabled_by_env(&environment) {
        return UpdatePreflightResult::Continue;
    }
    run_update_preflight_inner(runtime, current_version)
        .await
        .unwrap_or(UpdatePreflightResult::Continue)
}

async fn run_update_preflight_inner<R>(
    runtime: Arc<R>,
    current_version: &str,
) -> Result<UpdatePreflightResult, UpdatePreflightError>
where
    R: UpdatePreflightRuntime,
{
    let is_interactive = runtime.is_interactive();
    let device_id = runtime.device_id();
    let environment = runtime.environment();
    let bypass_rollout = is_rollout_bypassed_by_experimental_env(&environment);
    let mut install_state = runtime
        .read_install_state()
        .await
        .unwrap_or_else(|_| empty_update_install_state());
    if is_interactive {
        install_state = runtime
            .show_pending_background_install_notice(&install_state, current_version)
            .await?;
    }

    let cache = runtime
        .read_update_cache()
        .await
        .unwrap_or_else(|_| empty_update_cache());
    let cached_decision = decide_passive_update_target(
        current_version,
        cache.latest.as_deref(),
        cache.manifest.as_ref(),
        &device_id,
        runtime.now(),
        bypass_rollout,
    );
    log_rollout_decision(
        runtime.as_ref(),
        "startup-cache",
        current_version,
        cache.latest.as_deref(),
        cache.manifest.as_ref(),
        &cached_decision,
    );
    let Some(target) = cached_decision.target else {
        spawn_refresh_and_maybe_install(
            runtime,
            current_version.to_owned(),
            device_id,
            bypass_rollout,
            is_interactive,
            install_state,
        );
        return Ok(UpdatePreflightResult::Continue);
    };

    let source = if is_interactive {
        runtime
            .detect_install_source()
            .await
            .unwrap_or(InstallSource::Unsupported)
    } else {
        InstallSource::Unsupported
    };
    let platform = runtime.platform();
    let decision = decide_update_action(Some(&target), is_interactive, source, platform);
    if decision == UpdateDecision::None {
        spawn_refresh(runtime);
        return Ok(UpdatePreflightResult::Continue);
    }

    let cached_rollout = rollout_telemetry_for(
        &device_id,
        &target.version,
        cache.manifest.as_ref(),
        bypass_rollout,
    );
    if runtime
        .try_start_automatic_background_install(
            &install_state,
            current_version,
            &target,
            source,
            &cached_rollout,
        )
        .await
    {
        spawn_refresh(runtime);
        return Ok(UpdatePreflightResult::Continue);
    }

    let visible = refresh_user_visible_target(
        Arc::clone(&runtime),
        current_version.to_owned(),
        device_id.clone(),
        bypass_rollout,
        target.clone(),
        cache.manifest.clone(),
    )
    .await;
    let Some(visible_target) = visible.target else {
        return Ok(UpdatePreflightResult::Continue);
    };
    let visible_rollout = rollout_telemetry_for(
        &device_id,
        &visible_target.version,
        visible.manifest.as_ref(),
        bypass_rollout,
    );
    if runtime
        .try_start_automatic_background_install(
            &install_state,
            current_version,
            &visible_target,
            source,
            &visible_rollout,
        )
        .await
    {
        return Ok(UpdatePreflightResult::Continue);
    }

    let install_command = install_command_for(source, &visible_target.version, platform);
    track_update_prompted(
        runtime.as_ref(),
        current_version,
        &visible_target,
        source,
        decision,
        &visible_rollout,
    );
    if decision == UpdateDecision::ManualCommand {
        runtime.write_stdout(&super::preflight::render_manual_update_message(
            current_version,
            &visible_target,
            source,
            &install_command,
        ));
        return Ok(UpdatePreflightResult::Continue);
    }

    let choice = runtime
        .prompt_for_install_choice(&InstallPromptOptions {
            current_version: current_version.to_owned(),
            target: visible_target.clone(),
            install_source: source,
            install_command,
        })
        .await?;
    if choice == InstallPromptChoiceValue::Skip {
        return Ok(UpdatePreflightResult::Continue);
    }
    match runtime
        .install_update(source, &visible_target.version)
        .await
    {
        Ok(()) => {
            runtime.write_stdout(&super::preflight::render_install_success_message(
                &visible_target,
            ));
            Ok(UpdatePreflightResult::Exit)
        }
        Err(error) => {
            runtime.write_stderr(&format!(
                "warning: failed to install {NPM_PACKAGE_NAME}@{}: {error}\n",
                visible_target.version
            ));
            Ok(UpdatePreflightResult::Continue)
        }
    }
}

#[derive(Clone)]
struct VisibleUpdate {
    target: Option<UpdateTarget>,
    manifest: Option<UpdateManifest>,
}

async fn refresh_user_visible_target<R: UpdatePreflightRuntime>(
    runtime: Arc<R>,
    current_version: String,
    device_id: String,
    bypass_rollout: bool,
    fallback_target: UpdateTarget,
    fallback_manifest: Option<UpdateManifest>,
) -> VisibleUpdate {
    let fallback = VisibleUpdate {
        target: Some(fallback_target),
        manifest: fallback_manifest,
    };
    let task_fallback = fallback.clone();
    let (send, receive) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let visible = match runtime.refresh_update_cache().await {
            Ok(cache) => {
                let decision = decide_passive_update_target(
                    &current_version,
                    cache.latest.as_deref(),
                    cache.manifest.as_ref(),
                    &device_id,
                    runtime.now(),
                    bypass_rollout,
                );
                log_rollout_decision(
                    runtime.as_ref(),
                    "prompt-refresh",
                    &current_version,
                    cache.latest.as_deref(),
                    cache.manifest.as_ref(),
                    &decision,
                );
                VisibleUpdate {
                    target: decision.target,
                    manifest: cache.manifest,
                }
            }
            Err(_) => task_fallback,
        };
        let _ = send.send(visible);
    });
    match tokio::time::timeout(
        super::preflight::USER_VISIBLE_UPDATE_REFRESH_TIMEOUT,
        receive,
    )
    .await
    {
        Ok(Ok(visible)) => visible,
        Ok(Err(_)) | Err(_) => fallback,
    }
}

fn spawn_refresh<R: UpdatePreflightRuntime>(runtime: Arc<R>) {
    tokio::spawn(async move {
        let _ = runtime.refresh_update_cache().await;
    });
}

fn spawn_refresh_and_maybe_install<R: UpdatePreflightRuntime>(
    runtime: Arc<R>,
    current_version: String,
    device_id: String,
    bypass_rollout: bool,
    is_interactive: bool,
    install_state: UpdateInstallState,
) {
    tokio::spawn(async move {
        let Ok(cache) = runtime.refresh_update_cache().await else {
            return;
        };
        if !is_interactive {
            return;
        }
        let decision = decide_passive_update_target(
            &current_version,
            cache.latest.as_deref(),
            cache.manifest.as_ref(),
            &device_id,
            runtime.now(),
            bypass_rollout,
        );
        log_rollout_decision(
            runtime.as_ref(),
            "background-refresh",
            &current_version,
            cache.latest.as_deref(),
            cache.manifest.as_ref(),
            &decision,
        );
        let Some(target) = decision.target else {
            return;
        };
        let source = runtime
            .detect_install_source()
            .await
            .unwrap_or(InstallSource::Unsupported);
        let rollout = rollout_telemetry_for(
            &device_id,
            &target.version,
            cache.manifest.as_ref(),
            bypass_rollout,
        );
        let _ = runtime
            .try_start_automatic_background_install(
                &install_state,
                &current_version,
                &target,
                source,
                &rollout,
            )
            .await;
    });
}

fn track_update_prompted<R: UpdatePreflightRuntime>(
    runtime: &R,
    current_version: &str,
    target: &UpdateTarget,
    source: InstallSource,
    decision: UpdateDecision,
    rollout: &RolloutTelemetry,
) {
    runtime.track(
        "update_prompted",
        &object([
            ("current_version", json!(current_version)),
            ("target_version", json!(target.version)),
            ("source", json!(source.as_str())),
            ("decision", json!(decision_name(decision))),
            ("rollout_bucket", json!(rollout.rollout_bucket)),
            (
                "rollout_delay_seconds",
                json!(rollout.rollout_delay_seconds),
            ),
            (
                "rollout_from_manifest",
                json!(rollout.rollout_from_manifest),
            ),
            ("rollout_bypassed", json!(rollout.rollout_bypassed)),
        ]),
    );
}

fn log_rollout_decision<R: UpdatePreflightRuntime>(
    runtime: &R,
    phase: &str,
    current_version: &str,
    latest: Option<&str>,
    manifest: Option<&UpdateManifest>,
    decision: &PassiveUpdateDecision,
) {
    runtime.append_rollout_decision_log(object([
        (
            "ts",
            json!(runtime.now().to_rfc3339_opts(SecondsFormat::Millis, true)),
        ),
        ("phase", json!(phase)),
        ("reason", json!(decision.reason)),
        ("current", json!(current_version)),
        ("latest", json!(latest)),
        (
            "target",
            json!(decision.target.as_ref().map(|target| &target.version)),
        ),
        ("manifestPresent", json!(manifest.is_some())),
        (
            "publishedAt",
            json!(manifest.map(|manifest| &manifest.published_at)),
        ),
        ("bucket", json!(decision.bucket)),
        ("delaySeconds", json!(decision.delay_seconds)),
        ("eligibleAt", json!(decision.eligible_at)),
    ]));
}

fn decision_name(decision: UpdateDecision) -> &'static str {
    match decision {
        UpdateDecision::None => "none",
        UpdateDecision::PromptInstall => "prompt-install",
        UpdateDecision::ManualCommand => "manual-command",
    }
}

fn object<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use chrono::TimeZone;

    use super::*;
    use crate::cli::update::types::UpdateCacheSource;

    struct RuntimeMock {
        environment: HashMap<String, String>,
        interactive: bool,
        source: Result<InstallSource, &'static str>,
        install_state: Result<UpdateInstallState, &'static str>,
        cache: Result<UpdateCache, &'static str>,
        refresh_gate: Option<Arc<tokio::sync::Notify>>,
        refreshes: Mutex<VecDeque<Result<UpdateCache, &'static str>>>,
        automatic: Mutex<VecDeque<bool>>,
        prompt: Result<InstallPromptChoiceValue, &'static str>,
        install: Result<(), &'static str>,
        operations: Mutex<Vec<String>>,
        prompts: Mutex<Vec<InstallPromptOptions>>,
        events: Mutex<Vec<(String, Map<String, Value>)>>,
        logs: Mutex<Vec<Map<String, Value>>>,
        stdout: Mutex<String>,
        stderr: Mutex<String>,
    }

    impl RuntimeMock {
        fn new(cache: UpdateCache) -> Self {
            Self {
                environment: HashMap::new(),
                interactive: true,
                source: Ok(InstallSource::NpmGlobal),
                install_state: Ok(empty_update_install_state()),
                cache: Ok(cache),
                refresh_gate: None,
                refreshes: Mutex::new(VecDeque::from([Ok(cache_with("0.5.0"))])),
                automatic: Mutex::new(VecDeque::from([false, false])),
                prompt: Ok(InstallPromptChoiceValue::Skip),
                install: Ok(()),
                operations: Mutex::new(Vec::new()),
                prompts: Mutex::new(Vec::new()),
                events: Mutex::new(Vec::new()),
                logs: Mutex::new(Vec::new()),
                stdout: Mutex::new(String::new()),
                stderr: Mutex::new(String::new()),
            }
        }

        fn record(&self, operation: &str) {
            self.operations
                .lock()
                .expect("operations")
                .push(operation.to_owned());
        }
    }

    #[async_trait]
    impl UpdatePreflightRuntime for RuntimeMock {
        fn environment(&self) -> HashMap<String, String> {
            self.environment.clone()
        }

        fn is_interactive(&self) -> bool {
            self.interactive
        }

        fn device_id(&self) -> String {
            "test-device".to_owned()
        }

        fn platform(&self) -> UpdatePlatform {
            UpdatePlatform::Other
        }

        fn now(&self) -> DateTime<Utc> {
            Utc.with_ymd_and_hms(2026, 7, 21, 12, 0, 0)
                .single()
                .expect("time")
        }

        async fn read_install_state(&self) -> Result<UpdateInstallState, UpdatePreflightError> {
            self.record("read-install-state");
            self.install_state
                .clone()
                .map_err(UpdatePreflightError::message)
        }

        async fn show_pending_background_install_notice(
            &self,
            state: &UpdateInstallState,
            _: &str,
        ) -> Result<UpdateInstallState, UpdatePreflightError> {
            self.record("show-notice");
            Ok(state.clone())
        }

        async fn read_update_cache(&self) -> Result<UpdateCache, UpdatePreflightError> {
            self.record("read-cache");
            self.cache.clone().map_err(UpdatePreflightError::message)
        }

        async fn refresh_update_cache(&self) -> Result<UpdateCache, UpdatePreflightError> {
            self.record("refresh");
            if let Some(gate) = &self.refresh_gate {
                gate.notified().await;
            }
            self.refreshes
                .lock()
                .expect("refreshes")
                .pop_front()
                .unwrap_or(Err("unexpected refresh"))
                .map_err(UpdatePreflightError::message)
        }

        async fn detect_install_source(&self) -> Result<InstallSource, UpdatePreflightError> {
            self.record("detect-source");
            self.source.map_err(UpdatePreflightError::message)
        }

        async fn try_start_automatic_background_install(
            &self,
            _: &UpdateInstallState,
            _: &str,
            target: &UpdateTarget,
            _: InstallSource,
            _: &RolloutTelemetry,
        ) -> bool {
            self.record(&format!("auto:{}", target.version));
            self.automatic
                .lock()
                .expect("automatic")
                .pop_front()
                .unwrap_or(false)
        }

        async fn prompt_for_install_choice(
            &self,
            options: &InstallPromptOptions,
        ) -> Result<InstallPromptChoiceValue, UpdatePreflightError> {
            self.record("prompt");
            self.prompts.lock().expect("prompts").push(options.clone());
            self.prompt.map_err(UpdatePreflightError::message)
        }

        async fn install_update(
            &self,
            _: InstallSource,
            version: &str,
        ) -> Result<(), UpdatePreflightError> {
            self.record(&format!("install:{version}"));
            self.install.map_err(UpdatePreflightError::message)
        }

        fn append_rollout_decision_log(&self, entry: Map<String, Value>) {
            self.logs.lock().expect("logs").push(entry);
        }

        fn track(&self, event: &str, properties: &Map<String, Value>) {
            self.events
                .lock()
                .expect("events")
                .push((event.to_owned(), properties.clone()));
        }

        fn write_stdout(&self, text: &str) {
            self.stdout.lock().expect("stdout").push_str(text);
        }

        fn write_stderr(&self, text: &str) {
            self.stderr.lock().expect("stderr").push_str(text);
        }
    }

    fn cache_with(version: &str) -> UpdateCache {
        UpdateCache {
            source: UpdateCacheSource::Cdn,
            checked_at: Some("2026-07-21T12:00:00.000Z".to_owned()),
            latest: Some(version.to_owned()),
            manifest: None,
        }
    }

    async fn settle_background_tasks() {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn disabled_updates_skip_every_preflight_dependency() {
        let mut runtime = RuntimeMock::new(cache_with("0.5.0"));
        runtime
            .environment
            .insert("KIMI_CODE_NO_AUTO_UPDATE".to_owned(), "yes".to_owned());
        let runtime = Arc::new(runtime);

        assert_eq!(
            run_update_preflight(runtime.clone(), "0.4.0").await,
            UpdatePreflightResult::Continue
        );
        assert!(runtime.operations.lock().expect("operations").is_empty());
    }

    #[tokio::test]
    async fn empty_cache_refreshes_and_attempts_background_install_only_when_interactive() {
        let runtime = Arc::new(RuntimeMock::new(empty_update_cache()));
        assert_eq!(
            run_update_preflight(runtime.clone(), "0.4.0").await,
            UpdatePreflightResult::Continue
        );
        settle_background_tasks().await;
        let operations = runtime.operations.lock().expect("operations").clone();
        assert!(operations.ends_with(&[
            "refresh".to_owned(),
            "detect-source".to_owned(),
            "auto:0.5.0".to_owned()
        ]));

        let mut headless = RuntimeMock::new(empty_update_cache());
        headless.interactive = false;
        let headless = Arc::new(headless);
        run_update_preflight(headless.clone(), "0.4.0").await;
        settle_background_tasks().await;
        let operations = headless.operations.lock().expect("operations");
        assert!(operations.contains(&"refresh".to_owned()));
        assert!(!operations.contains(&"detect-source".to_owned()));
    }

    #[tokio::test]
    async fn automatic_cached_install_short_circuits_prompt_and_refreshes_cache() {
        let runtime = RuntimeMock::new(cache_with("0.5.0"));
        *runtime.automatic.lock().expect("automatic") = VecDeque::from([true]);
        let runtime = Arc::new(runtime);

        assert_eq!(
            run_update_preflight(runtime.clone(), "0.4.0").await,
            UpdatePreflightResult::Continue
        );
        settle_background_tasks().await;
        let operations = runtime.operations.lock().expect("operations");
        assert!(operations.contains(&"auto:0.5.0".to_owned()));
        assert!(operations.contains(&"refresh".to_owned()));
        assert!(!operations.contains(&"prompt".to_owned()));
    }

    #[tokio::test]
    async fn fresh_target_is_used_for_prompt_install_and_success_exits() {
        let mut runtime = RuntimeMock::new(cache_with("0.5.0"));
        *runtime.refreshes.lock().expect("refreshes") = VecDeque::from([Ok(cache_with("0.7.0"))]);
        runtime.prompt = Ok(InstallPromptChoiceValue::Install);
        let runtime = Arc::new(runtime);

        assert_eq!(
            run_update_preflight(runtime.clone(), "0.4.0").await,
            UpdatePreflightResult::Exit
        );
        let prompts = runtime.prompts.lock().expect("prompts");
        assert_eq!(prompts[0].target.version, "0.7.0");
        assert_eq!(
            prompts[0].install_command,
            "npm install -g @moonshot-ai/kimi-code@0.7.0"
        );
        assert!(
            runtime
                .stdout
                .lock()
                .expect("stdout")
                .contains("Updated @moonshot-ai/kimi-code to 0.7.0")
        );
        let events = runtime.events.lock().expect("events");
        assert_eq!(events[0].0, "update_prompted");
        assert_eq!(events[0].1["target_version"], json!("0.7.0"));
    }

    #[tokio::test(start_paused = true)]
    async fn timed_out_prompt_refresh_keeps_running_and_uses_cached_fallback() {
        let mut runtime = RuntimeMock::new(cache_with("0.6.0"));
        let gate = Arc::new(tokio::sync::Notify::new());
        runtime.refresh_gate = Some(gate.clone());
        *runtime.refreshes.lock().expect("refreshes") = VecDeque::from([Ok(cache_with("0.7.0"))]);
        let runtime = Arc::new(runtime);

        let preflight = tokio::spawn(run_update_preflight(runtime.clone(), "0.5.0"));
        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        assert_eq!(
            preflight.await.expect("preflight task"),
            UpdatePreflightResult::Continue
        );
        assert_eq!(
            runtime.prompts.lock().expect("prompts")[0].target.version,
            "0.6.0"
        );

        gate.notify_one();
        settle_background_tasks().await;
        assert!(runtime.logs.lock().expect("logs").iter().any(|entry| {
            entry.get("phase") == Some(&Value::String("prompt-refresh".to_owned()))
                && entry.get("target") == Some(&Value::String("0.7.0".to_owned()))
        }));
    }

    #[tokio::test]
    async fn withdrawn_refresh_suppresses_prompt_and_manual_source_prints_command() {
        let runtime = RuntimeMock::new(cache_with("0.5.0"));
        *runtime.refreshes.lock().expect("refreshes") = VecDeque::from([Ok(cache_with("0.4.0"))]);
        let runtime = Arc::new(runtime);
        run_update_preflight(runtime.clone(), "0.4.0").await;
        assert!(runtime.prompts.lock().expect("prompts").is_empty());
        assert!(runtime.stdout.lock().expect("stdout").is_empty());

        let mut manual = RuntimeMock::new(cache_with("0.5.0"));
        manual.source = Ok(InstallSource::Homebrew);
        let manual = Arc::new(manual);
        run_update_preflight(manual.clone(), "0.4.0").await;
        assert!(
            manual
                .stdout
                .lock()
                .expect("stdout")
                .contains("brew upgrade kimi-code")
        );
        assert!(manual.prompts.lock().expect("prompts").is_empty());
    }

    #[tokio::test]
    async fn skipped_and_failed_foreground_installs_continue_without_false_success() {
        let skipped = Arc::new(RuntimeMock::new(cache_with("0.5.0")));
        assert_eq!(
            run_update_preflight(skipped.clone(), "0.4.0").await,
            UpdatePreflightResult::Continue
        );
        assert!(!skipped.stdout.lock().expect("stdout").contains("Updated"));

        let mut failed = RuntimeMock::new(cache_with("0.5.0"));
        failed.prompt = Ok(InstallPromptChoiceValue::Install);
        failed.install = Err("npm exited with code 1");
        let failed = Arc::new(failed);
        assert_eq!(
            run_update_preflight(failed.clone(), "0.4.0").await,
            UpdatePreflightResult::Continue
        );
        assert!(
            failed
                .stderr
                .lock()
                .expect("stderr")
                .contains("npm exited with code 1")
        );
        assert!(!failed.stdout.lock().expect("stdout").contains("Updated"));
    }
}
