use std::sync::{Arc, Mutex};

use crate::{
    _base::di::lifecycle::{Disposable, DisposableHandle, DisposeResult},
    agent::{
        context_injector::{
            AgentContextInjectorServiceContract, ContextInjectionContent, ContextInjectionContext,
            ContextInjectionProvider, ContextInjectionResult,
        },
        permission_policy::PermissionMode,
    },
};

const PERMISSION_MODE_INJECTION_VARIANT: &str = "permission_mode";
const AUTO_MODE_ENTER_REMINDER: &str = include_str!("permission-mode-auto-enter-reminder.md");
const AUTO_MODE_EXIT_REMINDER: &str = include_str!("permission-mode-auto-exit-reminder.md");

pub trait PermissionModeReader: Send + Sync {
    fn mode(&self) -> PermissionMode;
}

// Original:
//   packages/agent-core-v2/src/agent/permissionMode/injection/permissionModeInjection.ts
//   PermissionModeInjection
pub struct PermissionModeInjection {
    registration: DisposableHandle,
}

impl PermissionModeInjection {
    pub fn new(
        permission_mode: Arc<dyn PermissionModeReader>,
        injector: &dyn AgentContextInjectorServiceContract,
    ) -> Self {
        let last_mode = Arc::new(Mutex::new(None));
        let provider: ContextInjectionProvider = Arc::new(move |context| {
            let current_mode = permission_mode.mode();
            let reminder =
                permission_mode_reminder(current_mode, &mut last_mode.lock().unwrap(), &context);
            Box::pin(async move {
                Ok(reminder.map(ContextInjectionContent::Text)) as ContextInjectionResult
            })
        });
        Self {
            registration: injector.register(PERMISSION_MODE_INJECTION_VARIANT.into(), provider),
        }
    }
}

impl Disposable for PermissionModeInjection {
    fn dispose(&self) -> DisposeResult {
        self.registration.dispose()
    }
}

// Original: permissionModeInjection.ts, reminder().
pub fn permission_mode_reminder(
    current_mode: PermissionMode,
    last_mode: &mut Option<PermissionMode>,
    context: &ContextInjectionContext,
) -> Option<String> {
    let previous_mode = *last_mode;
    if Some(current_mode) == previous_mode {
        if !context.injected_positions.is_empty() || current_mode != PermissionMode::Auto {
            return None;
        }
        return Some(AUTO_MODE_ENTER_REMINDER.to_owned());
    }
    *last_mode = Some(current_mode);
    if current_mode == PermissionMode::Auto {
        return Some(AUTO_MODE_ENTER_REMINDER.to_owned());
    }
    if previous_mode == Some(PermissionMode::Auto) {
        return Some(AUTO_MODE_EXIT_REMINDER.to_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use async_trait::async_trait;

    use super::*;
    use crate::{
        _base::di::lifecycle::to_disposable, agent::context_injector::ContextInjectionError,
    };

    fn context(positions: &[usize]) -> ContextInjectionContext {
        ContextInjectionContext {
            injected_positions: positions.to_vec(),
            last_injected_at: positions.last().copied(),
            is_new_turn: true,
        }
    }

    #[test]
    fn reminder_tracks_mode_transitions_and_history_derived_dedup() {
        let mut last = None;
        assert_eq!(
            permission_mode_reminder(PermissionMode::Manual, &mut last, &context(&[])),
            None
        );
        let enter =
            permission_mode_reminder(PermissionMode::Auto, &mut last, &context(&[])).unwrap();
        assert!(enter.contains("Auto permission mode is active"));
        assert!(enter.contains("ExitPlanMode is also approved automatically"));
        assert_eq!(
            permission_mode_reminder(PermissionMode::Auto, &mut last, &context(&[2])),
            None
        );
        assert!(
            permission_mode_reminder(PermissionMode::Auto, &mut last, &context(&[]))
                .unwrap()
                .contains("Auto permission mode is active")
        );
        assert!(
            permission_mode_reminder(PermissionMode::Manual, &mut last, &context(&[]))
                .unwrap()
                .contains("no longer active")
        );
        assert_eq!(
            permission_mode_reminder(PermissionMode::Yolo, &mut last, &context(&[])),
            None
        );
    }

    struct Mode(Mutex<PermissionMode>);

    impl PermissionModeReader for Mode {
        fn mode(&self) -> PermissionMode {
            *self.0.lock().unwrap()
        }
    }

    #[derive(Default)]
    struct Injector {
        provider: Mutex<Option<ContextInjectionProvider>>,
        disposed: Arc<AtomicBool>,
    }

    #[async_trait]
    impl AgentContextInjectorServiceContract for Injector {
        fn register(&self, name: String, provider: ContextInjectionProvider) -> DisposableHandle {
            assert_eq!(name, PERMISSION_MODE_INJECTION_VARIANT);
            *self.provider.lock().unwrap() = Some(provider);
            let disposed = Arc::clone(&self.disposed);
            to_disposable(move || {
                disposed.store(true, Ordering::SeqCst);
            })
        }

        async fn inject_after_compaction(&self) -> Result<(), ContextInjectionError> {
            Ok(())
        }
    }

    impl Disposable for Injector {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    #[tokio::test]
    async fn injection_registers_provider_and_disposes_registration() {
        let mode = Arc::new(Mode(Mutex::new(PermissionMode::Auto)));
        let injector = Injector::default();
        let mode_reader: Arc<dyn PermissionModeReader> = mode;
        let injection = PermissionModeInjection::new(mode_reader, &injector);
        let provider = injector.provider.lock().unwrap().clone().unwrap();
        let content = provider(context(&[])).await.unwrap().unwrap();
        assert!(matches!(
            content,
            ContextInjectionContent::Text(text) if text.contains("Auto permission mode is active")
        ));
        injection.dispose().unwrap();
        assert!(injector.disposed.load(Ordering::SeqCst));
    }
}
