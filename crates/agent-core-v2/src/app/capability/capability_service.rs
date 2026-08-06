//! `capability` domain (L3) — `CapabilityServiceContract` implementation.
//!
//! Holds the closed registry of built-in capability entries and serializes
//! install runs per entry. Install progress lives in memory only and is
//! polled by clients; a failed attempt leaves its error in the progress state
//! until the next attempt starts. Listing degrades a single entry's failing
//! detection to a failed step on that entry instead of rejecting the whole
//! list. Bound at App scope.
//!
//! Original: `packages/agent-core-v2/src/app/capability/capabilityService.ts`.

use std::{
    collections::HashMap,
    error::Error,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::{Error2, Error2Options},
    },
    app::{
        bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
        plugin::{PLUGIN_SERVICE_ID, PluginServiceHandle},
    },
    os::interface::host_process::{HOST_PROCESS_SERVICE_ID, HostProcessServiceHandle},
};

use super::{
    contract::{
        CAPABILITY_SERVICE_ID, CapabilityServiceContract, CapabilityServiceHandle,
        CapabilityServiceResult,
    },
    entries::{CapabilityEntryContext, create_kimi_cu_entry, create_kimi_webbridge_entry},
    errors::{
        CAPABILITY_INSTALL_IN_PROGRESS, CAPABILITY_NOT_FOUND, CAPABILITY_UNSUPPORTED,
        ensure_capability_errors_registered,
    },
    types::{
        CapabilityEntry, CapabilityId, CapabilityInstallProgress, CapabilityInstallReporter,
        CapabilityReadiness, CapabilityStatus, CapabilityStep, CapabilityStepState,
    },
};

pub struct CapabilityService {
    // IndexMap keeps the registry order stable for `listCapabilities()` —
    // node Maps iterate in insertion order.
    entries: IndexMap<CapabilityId, Arc<dyn CapabilityEntry>>,
    install_progress: Arc<Mutex<HashMap<CapabilityId, CapabilityInstallProgress>>>,
    platform: String,
    arch: String,
}

impl CapabilityService {
    pub fn new(
        bootstrap: BootstrapServiceHandle,
        plugins: PluginServiceHandle,
        host_process: HostProcessServiceHandle,
    ) -> Self {
        let ctx = CapabilityEntryContext {
            platform: bootstrap.platform().to_owned(),
            arch: bootstrap.arch().to_owned(),
            kimi_home_dir: bootstrap.home_dir().to_owned(),
            user_home_dir: bootstrap.os_home_dir().to_owned(),
            plugins,
            host_process,
            fetch_impl: None,
            applications_dir: None,
            webbridge_base_url: None,
            detect_probe_timeout: None,
            command_timeout: None,
        };
        let platform = ctx.platform.clone();
        let arch = ctx.arch.clone();
        Self {
            entries: IndexMap::from([
                (CapabilityId::KimiCu, create_kimi_cu_entry(ctx.clone())),
                (CapabilityId::KimiWebbridge, create_kimi_webbridge_entry(ctx)),
            ]),
            install_progress: Arc::new(Mutex::new(HashMap::new())),
            platform,
            arch,
        }
    }

    /// Original: the `entriesOverride` constructor parameter — tests inject
    /// fake entries instead of the built-in registry.
    pub fn with_entries(entries: Vec<Arc<dyn CapabilityEntry>>) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|entry| (entry.id(), entry))
                .collect(),
            install_progress: Arc::new(Mutex::new(HashMap::new())),
            platform: node_platform(),
            arch: node_arch(),
        }
    }

    fn progress_of(&self, id: CapabilityId) -> CapabilityInstallProgress {
        self.install_progress
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap_or_default()
    }

    fn require_entry(&self, id: &str) -> CapabilityServiceResult<Arc<dyn CapabilityEntry>> {
        let entry = CapabilityId::try_from(id)
            .ok()
            .and_then(|id| self.entries.get(&id));
        entry.cloned().ok_or_else(|| {
            coded_error(
                CAPABILITY_NOT_FOUND,
                format!("Capability \"{id}\" is not registered"),
                id,
            )
        })
    }

    async fn status_of(
        &self,
        entry: &Arc<dyn CapabilityEntry>,
    ) -> CapabilityServiceResult<CapabilityStatus> {
        let install = self.progress_of(entry.id());
        if !entry.supported() {
            return Ok(CapabilityStatus {
                id: entry.id(),
                plugin_id: entry.plugin_id().map(str::to_owned),
                display_name: entry.display_name().to_owned(),
                description: entry.description().to_owned(),
                supported: false,
                state: CapabilityReadiness::Unsupported,
                version: None,
                steps: Vec::new(),
                install,
            });
        }
        let detected = entry
            .detect()
            .await
            .map_err(|error| error as Box<dyn Error + Send + Sync>)?;
        let mut required = detected
            .steps
            .iter()
            .filter(|step| step.optional != Some(true))
            .peekable();
        let required_ok = required.peek().is_some()
            && required.all(|step| step.state == CapabilityStepState::Ok);
        let any_ok = detected
            .steps
            .iter()
            .any(|step| step.state == CapabilityStepState::Ok);
        let state = if required_ok {
            CapabilityReadiness::Ready
        } else if any_ok {
            CapabilityReadiness::Partial
        } else {
            CapabilityReadiness::NotInstalled
        };
        Ok(CapabilityStatus {
            id: entry.id(),
            plugin_id: entry.plugin_id().map(str::to_owned),
            display_name: entry.display_name().to_owned(),
            description: entry.description().to_owned(),
            supported: true,
            state,
            version: detected.version,
            steps: detected.steps,
            install,
        })
    }

    // Original: statusOfSafe() — one entry's broken probe must not take down
    // the whole list.
    async fn status_of_safe(&self, entry: &Arc<dyn CapabilityEntry>) -> CapabilityStatus {
        match self.status_of(entry).await {
            Ok(status) => status,
            Err(error) => {
                let install = self.progress_of(entry.id());
                CapabilityStatus {
                    id: entry.id(),
                    plugin_id: entry.plugin_id().map(str::to_owned),
                    display_name: entry.display_name().to_owned(),
                    description: entry.description().to_owned(),
                    supported: entry.supported(),
                    state: if entry.supported() {
                        CapabilityReadiness::Partial
                    } else {
                        CapabilityReadiness::Unsupported
                    },
                    version: None,
                    steps: if entry.supported() {
                        vec![CapabilityStep {
                            id: "detect".to_owned(),
                            state: CapabilityStepState::Failed,
                            detail: Some(error.to_string()),
                            optional: None,
                        }]
                    } else {
                        Vec::new()
                    },
                    install,
                }
            }
        }
    }
}

#[async_trait]
impl CapabilityServiceContract for CapabilityService {
    async fn list_capabilities(&self) -> CapabilityServiceResult<Vec<CapabilityStatus>> {
        Ok(
            futures_util::future::join_all(
                self.entries
                    .values()
                    .map(|entry| self.status_of_safe(entry)),
            )
            .await,
        )
    }

    async fn get_capability(&self, id: &str) -> CapabilityServiceResult<CapabilityStatus> {
        self.status_of(&self.require_entry(id)?).await
    }

    async fn install_capability(&self, id: &str) -> CapabilityServiceResult<CapabilityStatus> {
        let entry = self.require_entry(id)?;
        if !entry.supported() {
            return Err(coded_error(
                CAPABILITY_UNSUPPORTED,
                format!(
                    "Capability \"{}\" is not supported on {}/{}",
                    entry.id(),
                    self.platform,
                    self.arch
                ),
                entry.id().as_str(),
            ));
        }
        {
            let mut progress = self.install_progress.lock().unwrap();
            if progress.get(&entry.id()).is_some_and(|install| install.running) {
                return Err(coded_error(
                    CAPABILITY_INSTALL_IN_PROGRESS,
                    format!("Capability \"{}\" is already being installed", entry.id()),
                    entry.id().as_str(),
                ));
            }
            progress.insert(
                entry.id(),
                CapabilityInstallProgress {
                    running: true,
                    ..CapabilityInstallProgress::default()
                },
            );
        }

        let entry_id = entry.id();
        let reporter_progress = Arc::clone(&self.install_progress);
        let final_progress = Arc::clone(&self.install_progress);
        let installing = Arc::clone(&entry);
        tokio::spawn(async move {
            let report: CapabilityInstallReporter = Box::new(move |step, percent| {
                reporter_progress.lock().unwrap().insert(
                    entry_id,
                    CapabilityInstallProgress {
                        running: true,
                        step: Some(step.to_owned()),
                        percent,
                        error: None,
                    },
                );
            });
            let result = installing.install(report).await;
            let mut progress = final_progress.lock().unwrap();
            match result {
                Ok(()) => {
                    progress.insert(entry_id, CapabilityInstallProgress::default());
                }
                Err(error) => {
                    progress.insert(
                        entry_id,
                        CapabilityInstallProgress {
                            running: false,
                            step: None,
                            percent: None,
                            error: Some(error.to_string()),
                        },
                    );
                }
            }
        });

        self.status_of(&entry).await
    }
}

fn coded_error(
    code: &'static str,
    message: impl Into<String>,
    id: &str,
) -> Box<dyn Error + Send + Sync> {
    Box::new(Error2::with_options(
        code,
        message,
        Error2Options {
            details: Some(Map::from_iter([(
                "id".to_owned(),
                Value::String(id.to_owned()),
            )])),
            ..Error2Options::default()
        },
    ))
}

// Original: `process.platform` — the entry-override constructor has no
// bootstrap snapshot, so fall back to the host's node-style name.
fn node_platform() -> String {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        "illumos" => "sunos",
        other => other,
    }
    .into()
}

fn node_arch() -> String {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "x86" => "ia32",
        "aarch64" => "arm64",
        "loongarch64" => "loong64",
        "powerpc" => "ppc",
        "powerpc64" => "ppc64",
        other => other,
    }
    .into()
}

// Original: registerScopedService(..., ScopeActivation.OnScopeCreated, 'capability').
pub fn register_capability_service() {
    ensure_capability_errors_registered();
    register_scoped_service(
        LifecycleScope::App,
        CAPABILITY_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            let plugins = accessor.get(PLUGIN_SERVICE_ID)?;
            let host_process = accessor.get(HOST_PROCESS_SERVICE_ID)?;
            let service: Arc<dyn CapabilityServiceContract> = Arc::new(CapabilityService::new(
                (*bootstrap).clone(),
                (*plugins).clone(),
                (*host_process).clone(),
            ));
            Ok(CapabilityServiceHandle(service))
        }),
        InstantiationType::Eager,
        "capability",
    );
}

#[cfg(test)]
mod tests {
    //! `CapabilityService` — registry semantics, readiness computation, and
    //! install orchestration (progress transitions, serialized runs, coded
    //! errors). Entries are fakes; entry internals are covered per-entry.
    //!
    //! Original: `packages/agent-core-v2/test/app/capability/capabilityService.test.ts`.

    use std::{
        error::Error,
        pin::Pin,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    use crate::_base::errors::errors::ExpectedError;
    use crate::app::capability::{
        contract::CapabilityServiceError,
        types::{CapabilityDetectResult, CapabilityEntryResult},
    };

    use super::*;

    type InstallFuture =
        Pin<Box<dyn std::future::Future<Output = CapabilityEntryResult<()>> + Send>>;
    type InstallFn = Arc<dyn Fn(CapabilityInstallReporter) -> InstallFuture + Send + Sync>;

    struct FakeEntry {
        id: CapabilityId,
        plugin_id: Option<String>,
        supported: bool,
        detect: Result<CapabilityDetectResult, String>,
        install: InstallFn,
    }

    fn fake_entry(
        id: CapabilityId,
        plugin_id: Option<&str>,
        supported: bool,
        detect: Result<CapabilityDetectResult, &str>,
        install: Option<InstallFn>,
    ) -> Arc<dyn CapabilityEntry> {
        Arc::new(FakeEntry {
            id,
            plugin_id: plugin_id.map(str::to_owned),
            supported,
            detect: detect.map_err(str::to_owned),
            install: install
                .unwrap_or_else(|| Arc::new(|_| Box::pin(async { Ok(()) }))),
        })
    }

    fn ok_install() -> Option<InstallFn> {
        None
    }

    #[async_trait]
    impl CapabilityEntry for FakeEntry {
        fn id(&self) -> CapabilityId {
            self.id
        }

        fn plugin_id(&self) -> Option<&str> {
            self.plugin_id.as_deref()
        }

        fn display_name(&self) -> &str {
            self.id.as_str()
        }

        fn description(&self) -> &str {
            "fake"
        }

        fn supported(&self) -> bool {
            self.supported
        }

        async fn detect(&self) -> CapabilityEntryResult<CapabilityDetectResult> {
            match &self.detect {
                Ok(result) => Ok(result.clone()),
                Err(message) => Err(Box::new(ExpectedError::new(message.clone()))),
            }
        }

        async fn install(&self, report: CapabilityInstallReporter) -> CapabilityEntryResult<()> {
            (self.install)(report).await
        }
    }

    fn step(id: &str, state: CapabilityStepState) -> CapabilityStep {
        CapabilityStep::new(id, state)
    }

    fn optional_step(id: &str, state: CapabilityStepState) -> CapabilityStep {
        CapabilityStep {
            optional: Some(true),
            ..CapabilityStep::new(id, state)
        }
    }

    fn expect_error_code(error: CapabilityServiceError, code: &str) {
        let error2 = error.downcast_ref::<Error2>().expect("expected Error2");
        assert_eq!(error2.code, code);
    }

    async fn wait_for_install_to_settle(
        service: &CapabilityService,
        id: &str,
    ) -> CapabilityInstallProgress {
        for _ in 0..50 {
            let status = service.get_capability(id).await.unwrap();
            if !status.install.running {
                return status.install;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("install never settled");
    }

    #[tokio::test]
    async fn lists_entries_with_readiness_computed_from_required_steps() {
        let service = CapabilityService::with_entries(vec![
            fake_entry(
                CapabilityId::KimiCu,
                Some("kimi-cu-win"),
                true,
                Ok(CapabilityDetectResult {
                    version: None,
                    steps: vec![step("plugin", CapabilityStepState::Ok)],
                }),
                ok_install(),
            ),
            fake_entry(
                CapabilityId::KimiWebbridge,
                None,
                true,
                Ok(CapabilityDetectResult {
                    version: None,
                    steps: vec![
                        step("daemon", CapabilityStepState::Ok),
                        step("skill", CapabilityStepState::Missing),
                        optional_step("extension", CapabilityStepState::Missing),
                    ],
                }),
                ok_install(),
            ),
        ]);
        let list = service.list_capabilities().await.unwrap();
        let states: Vec<(CapabilityId, CapabilityReadiness)> = list
            .iter()
            .map(|capability| (capability.id, capability.state))
            .collect();
        assert_eq!(
            states,
            vec![
                (CapabilityId::KimiCu, CapabilityReadiness::Ready),
                (CapabilityId::KimiWebbridge, CapabilityReadiness::Partial),
            ]
        );
        assert_eq!(list[0].plugin_id.as_deref(), Some("kimi-cu-win"));
    }

    #[tokio::test]
    async fn isolates_a_failing_detector_to_its_own_entry() {
        let service = CapabilityService::with_entries(vec![
            fake_entry(
                CapabilityId::KimiCu,
                None,
                true,
                Err("probe timed out"),
                ok_install(),
            ),
            fake_entry(
                CapabilityId::KimiWebbridge,
                None,
                true,
                Ok(CapabilityDetectResult {
                    version: None,
                    steps: vec![step("daemon", CapabilityStepState::Ok)],
                }),
                ok_install(),
            ),
        ]);

        // One entry's broken probe must not take down the whole list.
        let list = service.list_capabilities().await.unwrap();
        let webbridge = list
            .iter()
            .find(|capability| capability.id == CapabilityId::KimiWebbridge)
            .unwrap();
        assert_eq!(webbridge.state, CapabilityReadiness::Ready);
        let cu = list
            .iter()
            .find(|capability| capability.id == CapabilityId::KimiCu)
            .unwrap();
        assert_eq!(cu.state, CapabilityReadiness::Partial);
        assert_eq!(
            cu.steps,
            vec![CapabilityStep {
                id: "detect".to_owned(),
                state: CapabilityStepState::Failed,
                detail: Some("probe timed out".to_owned()),
                optional: None,
            }]
        );
    }

    #[tokio::test]
    async fn marks_optional_steps_as_non_blocking_for_ready() {
        let service = CapabilityService::with_entries(vec![fake_entry(
            CapabilityId::KimiWebbridge,
            None,
            true,
            Ok(CapabilityDetectResult {
                version: Some("v1.11.3".to_owned()),
                steps: vec![
                    step("daemon", CapabilityStepState::Ok),
                    optional_step("extension", CapabilityStepState::Missing),
                ],
            }),
            ok_install(),
        )]);
        let status = service.get_capability("kimi-webbridge").await.unwrap();
        assert_eq!(status.state, CapabilityReadiness::Ready);
        assert_eq!(status.version.as_deref(), Some("v1.11.3"));
    }

    #[tokio::test]
    async fn reports_not_installed_when_no_step_is_ok_and_unsupported_as_is() {
        let service = CapabilityService::with_entries(vec![
            fake_entry(
                CapabilityId::KimiCu,
                None,
                true,
                Ok(CapabilityDetectResult {
                    version: None,
                    steps: vec![step("plugin", CapabilityStepState::Missing)],
                }),
                ok_install(),
            ),
            fake_entry(CapabilityId::KimiWebbridge, None, false, Ok(CapabilityDetectResult::default()), ok_install()),
        ]);
        let list = service.list_capabilities().await.unwrap();
        let cu = list
            .iter()
            .find(|capability| capability.id == CapabilityId::KimiCu)
            .unwrap();
        assert_eq!(cu.state, CapabilityReadiness::NotInstalled);
        let unsupported = list
            .iter()
            .find(|capability| capability.id == CapabilityId::KimiWebbridge)
            .unwrap();
        assert_eq!(unsupported.state, CapabilityReadiness::Unsupported);
        assert!(!unsupported.supported);
    }

    #[tokio::test]
    async fn throws_capability_not_found_for_unknown_ids() {
        let service = CapabilityService::with_entries(vec![]);
        let error = service.get_capability("nope").await.unwrap_err();
        expect_error_code(error, CAPABILITY_NOT_FOUND);
        let error = service.install_capability("nope").await.unwrap_err();
        expect_error_code(error, CAPABILITY_NOT_FOUND);
    }

    #[tokio::test]
    async fn rejects_install_on_an_unsupported_entry() {
        let service = CapabilityService::with_entries(vec![fake_entry(
            CapabilityId::KimiCu,
            None,
            false,
            Ok(CapabilityDetectResult::default()),
            ok_install(),
        )]);
        let error = service.install_capability("kimi-cu").await.unwrap_err();
        expect_error_code(error, CAPABILITY_UNSUPPORTED);
    }

    #[tokio::test]
    async fn serializes_installs_and_clears_progress_on_success() {
        let (release_sender, release_receiver) = tokio::sync::oneshot::channel::<()>();
        let release_receiver = Arc::new(Mutex::new(Some(release_receiver)));
        let install: InstallFn = Arc::new(move |report| {
            let release_receiver = Arc::clone(&release_receiver);
            Box::pin(async move {
                report("download", Some(42));
                let receiver = release_receiver.lock().unwrap().take().unwrap();
                receiver.await.ok();
                Ok(())
            })
        });
        let service = CapabilityService::with_entries(vec![fake_entry(
            CapabilityId::KimiCu,
            None,
            true,
            Ok(CapabilityDetectResult {
                version: None,
                steps: vec![step("plugin", CapabilityStepState::Ok)],
            }),
            Some(install),
        )]);

        let started = service.install_capability("kimi-cu").await.unwrap();
        assert!(started.install.running);

        let error = service.install_capability("kimi-cu").await.unwrap_err();
        expect_error_code(error, CAPABILITY_INSTALL_IN_PROGRESS);

        // The reporter runs inside the spawned task; wait until it lands.
        let mut during = started.clone();
        for _ in 0..50 {
            during = service.get_capability("kimi-cu").await.unwrap();
            if during.install.step.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            during.install,
            CapabilityInstallProgress {
                running: true,
                step: Some("download".to_owned()),
                percent: Some(42),
                error: None,
            }
        );

        release_sender.send(()).ok();
        let settled = wait_for_install_to_settle(&service, "kimi-cu").await;
        assert_eq!(settled.error, None);
    }

    #[tokio::test]
    async fn surfaces_install_errors_through_progress_until_the_next_attempt() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let counting = Arc::clone(&attempts);
        let install: InstallFn = Arc::new(move |_| {
            let counting = Arc::clone(&counting);
            Box::pin(async move {
                let attempt = counting.fetch_add(1, Ordering::SeqCst) + 1;
                if attempt == 1 {
                    Err(Box::new(ExpectedError::new("boom")) as Box<dyn Error + Send + Sync>)
                } else {
                    Ok(())
                }
            })
        });
        let service = CapabilityService::with_entries(vec![fake_entry(
            CapabilityId::KimiCu,
            None,
            true,
            Ok(CapabilityDetectResult {
                version: None,
                steps: vec![step("plugin", CapabilityStepState::Ok)],
            }),
            Some(install),
        )]);

        service.install_capability("kimi-cu").await.unwrap();
        let failed = wait_for_install_to_settle(&service, "kimi-cu").await;
        assert_eq!(
            failed,
            CapabilityInstallProgress {
                running: false,
                step: None,
                percent: None,
                error: Some("boom".to_owned()),
            }
        );

        // Retry clears the error.
        service.install_capability("kimi-cu").await.unwrap();
        let retried = wait_for_install_to_settle(&service, "kimi-cu").await;
        assert_eq!(retried.error, None);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
