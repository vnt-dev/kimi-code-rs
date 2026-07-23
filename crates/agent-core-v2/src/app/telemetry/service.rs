//! App-scoped telemetry fan-out service and forwarding context views.
//!
//! Original: `packages/agent-core-v2/src/app/telemetry/telemetryService.ts`.

use std::{
    any::Any,
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use futures_util::{FutureExt, future::join_all};

use crate::_base::{
    di::{
        descriptors::SyncDescriptor,
        lifecycle::{DisposableHandle, to_disposable},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    errors::unexpected_error::{on_unexpected_error, safely_call_listener},
};

use super::contract::{
    TELEMETRY_SERVICE_ID, TelemetryAppender, TelemetryContextPatch, TelemetryProperties,
    TelemetryServiceContract, TelemetryServiceHandle, null_telemetry_appender,
};

struct TelemetryState {
    appenders: Vec<Arc<dyn TelemetryAppender>>,
    context: TelemetryProperties,
    enabled: bool,
}

pub struct TelemetryService {
    state: Arc<Mutex<TelemetryState>>,
}

impl Default for TelemetryService {
    fn default() -> Self {
        Self::new()
    }
}

impl TelemetryService {
    // Original: TelemetryService field initializers.
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(TelemetryState {
                appenders: vec![null_telemetry_appender()],
                context: TelemetryProperties::new(),
                enabled: true,
            })),
        }
    }
}

#[async_trait]
impl TelemetryServiceContract for TelemetryService {
    // Original: TelemetryService.track(). The state snapshot ensures no lock is
    // held while user-provided appenders run.
    fn track(&self, event: &str, properties: Option<&TelemetryProperties>) {
        let (appenders, mut merged) = {
            let state = self.state.lock().unwrap();
            if !state.enabled {
                return;
            }
            (state.appenders.clone(), state.context.clone())
        };
        if let Some(properties) = properties {
            merged.extend(properties.clone());
        }
        for appender in appenders {
            safely_call_listener(|| appender.track(event, Some(&merged)));
        }
    }

    // Original: TelemetryService.withContext().
    fn with_context(&self, patch: &TelemetryContextPatch) -> TelemetryServiceHandle {
        let root: TelemetryServiceHandle = TelemetryServiceHandle(Arc::new(Self {
            state: Arc::clone(&self.state),
        }));
        TelemetryServiceHandle(Arc::new(TelemetryContextView::new(root, patch.clone())))
    }

    // Original: TelemetryService.setContext().
    fn set_context(&self, patch: &TelemetryContextPatch) {
        let appenders = {
            let mut state = self.state.lock().unwrap();
            state.context.extend(patch.clone());
            state.appenders.clone()
        };
        for appender in appenders {
            appender.set_context(patch);
        }
    }

    // Original: TelemetryService.addAppender().
    fn add_appender(&self, appender: Arc<dyn TelemetryAppender>) -> DisposableHandle {
        self.state
            .lock()
            .unwrap()
            .appenders
            .push(Arc::clone(&appender));
        let state = Arc::clone(&self.state);
        to_disposable(move || remove_from_state(&state, &appender))
    }

    // Original: TelemetryService.removeAppender(). All matching object
    // identities are removed, matching Array.filter(a => a !== appender).
    fn remove_appender(&self, appender: &Arc<dyn TelemetryAppender>) {
        remove_from_state(&self.state, appender);
    }

    // Original: TelemetryService.setAppender().
    fn set_appender(&self, appender: Arc<dyn TelemetryAppender>) {
        self.state.lock().unwrap().appenders = vec![appender];
    }

    // Original: TelemetryService.setEnabled().
    fn set_enabled(&self, enabled: bool) {
        self.state.lock().unwrap().enabled = enabled;
    }

    // Original: TelemetryService.flush(). Appenders start together like
    // Promise.all; one panicking appender is reported without rejecting the
    // service-level operation or preventing other appenders from completing.
    async fn flush(&self) {
        let appenders = self.state.lock().unwrap().appenders.clone();
        join_all(appenders.into_iter().map(|appender| async move {
            if let Err(payload) = AssertUnwindSafe(appender.flush()).catch_unwind().await {
                report_appender_panic("telemetry appender flush panicked", payload);
            }
        }))
        .await;
    }

    // Original: TelemetryService.shutdown().
    async fn shutdown(&self) {
        let appenders = self.state.lock().unwrap().appenders.clone();
        join_all(appenders.into_iter().map(|appender| async move {
            if let Err(payload) = AssertUnwindSafe(appender.shutdown()).catch_unwind().await {
                report_appender_panic("telemetry appender shutdown panicked", payload);
            }
        }))
        .await;
    }
}

fn remove_from_state(state: &Mutex<TelemetryState>, appender: &Arc<dyn TelemetryAppender>) {
    state
        .lock()
        .unwrap()
        .appenders
        .retain(|current| !Arc::ptr_eq(current, appender));
}

fn report_appender_panic(prefix: &str, payload: Box<dyn Any + Send>) {
    let detail = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic");
    on_unexpected_error(&std::io::Error::other(format!("{prefix}: {detail}")));
}

struct TelemetryContextView {
    root: TelemetryServiceHandle,
    context: Mutex<TelemetryProperties>,
}

impl TelemetryContextView {
    // Original: TelemetryContextView.constructor().
    fn new(root: TelemetryServiceHandle, context: TelemetryProperties) -> Self {
        Self {
            root,
            context: Mutex::new(context),
        }
    }
}

#[async_trait]
impl TelemetryServiceContract for TelemetryContextView {
    // Original: TelemetryContextView.track().
    fn track(&self, event: &str, properties: Option<&TelemetryProperties>) {
        let mut merged = self.context.lock().unwrap().clone();
        if let Some(properties) = properties {
            merged.extend(properties.clone());
        }
        self.root.track(event, Some(&merged));
    }

    // Original: TelemetryContextView.withContext().
    fn with_context(&self, patch: &TelemetryContextPatch) -> TelemetryServiceHandle {
        let mut merged = self.context.lock().unwrap().clone();
        merged.extend(patch.clone());
        TelemetryServiceHandle(Arc::new(Self::new(self.root.clone(), merged)))
    }

    // Original: TelemetryContextView.setContext().
    fn set_context(&self, patch: &TelemetryContextPatch) {
        self.context.lock().unwrap().extend(patch.clone());
    }

    fn add_appender(&self, appender: Arc<dyn TelemetryAppender>) -> DisposableHandle {
        self.root.add_appender(appender)
    }

    fn remove_appender(&self, appender: &Arc<dyn TelemetryAppender>) {
        self.root.remove_appender(appender);
    }

    fn set_appender(&self, appender: Arc<dyn TelemetryAppender>) {
        self.root.set_appender(appender);
    }

    fn set_enabled(&self, enabled: bool) {
        self.root.set_enabled(enabled);
    }

    async fn flush(&self) {
        self.root.flush().await;
    }

    async fn shutdown(&self) {
        self.root.shutdown().await;
    }
}

// Original: registerScopedService(... TelemetryService ...).
pub fn register_telemetry_service() {
    register_scoped_service(
        LifecycleScope::App,
        TELEMETRY_SERVICE_ID,
        SyncDescriptor::new(|_| {
            let service: Arc<dyn TelemetryServiceContract> = Arc::new(TelemetryService::new());
            Ok(TelemetryServiceHandle(service))
        }),
        InstantiationType::Eager,
        "telemetry",
    );
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::Value;

    use super::*;

    #[derive(Default)]
    struct CapturingAppender {
        events: Mutex<Vec<(String, TelemetryProperties)>>,
        flushes: AtomicUsize,
        shutdowns: AtomicUsize,
    }

    #[async_trait]
    impl TelemetryAppender for CapturingAppender {
        fn track(&self, event: &str, properties: Option<&TelemetryProperties>) {
            self.events
                .lock()
                .unwrap()
                .push((event.into(), properties.cloned().unwrap_or_default()));
        }

        async fn flush(&self) {
            self.flushes.fetch_add(1, Ordering::Relaxed);
        }

        async fn shutdown(&self) {
            self.shutdowns.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn properties(entries: &[(&str, Value)]) -> TelemetryProperties {
        entries
            .iter()
            .map(|(key, value)| ((*key).into(), Some(value.clone())))
            .collect()
    }

    #[tokio::test]
    async fn context_views_follow_root_state_and_disposable_removes_appender() {
        let root = TelemetryService::new();
        let appender = Arc::new(CapturingAppender::default());
        let erased: Arc<dyn TelemetryAppender> = appender.clone();
        root.set_appender(Arc::clone(&erased));
        root.set_context(&properties(&[("sessionId", Value::from("s1"))]));

        let child = root.with_context(&properties(&[
            ("agentId", Value::from("main")),
            ("turnId", Value::from("t1")),
        ]));
        child.track(
            "tool.call",
            Some(&properties(&[
                ("turnId", Value::from("override")),
                ("name", Value::from("bash")),
            ])),
        );
        assert_eq!(
            appender.events.lock().unwrap()[0].1,
            properties(&[
                ("sessionId", Value::from("s1")),
                ("agentId", Value::from("main")),
                ("turnId", Value::from("override")),
                ("name", Value::from("bash")),
            ])
        );

        root.set_enabled(false);
        child.track("dropped", None);
        root.set_enabled(true);
        let second = Arc::new(CapturingAppender::default());
        let second_erased: Arc<dyn TelemetryAppender> = second.clone();
        let disposable = child.add_appender(second_erased);
        child.track("both", None);
        disposable.dispose().unwrap();
        child.track("first-only", None);
        assert_eq!(appender.events.lock().unwrap().len(), 3);
        assert_eq!(second.events.lock().unwrap().len(), 1);

        child.flush().await;
        child.shutdown().await;
        assert_eq!(appender.flushes.load(Ordering::Relaxed), 1);
        assert_eq!(appender.shutdowns.load(Ordering::Relaxed), 1);
    }

    struct PanickingAppender;

    #[async_trait]
    impl TelemetryAppender for PanickingAppender {
        fn track(&self, _event: &str, _properties: Option<&TelemetryProperties>) {
            panic!("track boom");
        }

        async fn flush(&self) {
            panic!("flush boom");
        }

        async fn shutdown(&self) {
            panic!("shutdown boom");
        }
    }

    #[tokio::test]
    async fn panicking_appender_does_not_block_other_appenders() {
        let root = TelemetryService::new();
        root.set_appender(Arc::new(PanickingAppender));
        let good = Arc::new(CapturingAppender::default());
        let good_erased: Arc<dyn TelemetryAppender> = good.clone();
        root.add_appender(good_erased);

        root.track("event", None);
        root.flush().await;
        root.shutdown().await;

        assert_eq!(good.events.lock().unwrap().len(), 1);
        assert_eq!(good.flushes.load(Ordering::Relaxed), 1);
        assert_eq!(good.shutdowns.load(Ordering::Relaxed), 1);
    }
}
