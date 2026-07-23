use std::sync::{Arc, Mutex};

use crate::{_base::errors::errors::Error2, app::flag::FlagServiceHandle};

use super::{
    FAULT_INJECTION_FLAG_ID,
    contract::{FaultInjectionServiceContract, FaultInjectionStatus, FaultKind},
};

const REQUEST_INVALID: &str = "request.invalid";
const DISABLED_MESSAGE: &str = "Fault injection is disabled; enable the fault-injection experimental flag \
(KIMI_CODE_EXPERIMENTAL_FAULT_INJECTION=1, the master flag, or the \
[experimental] config section).";

/// Narrow adapter for the sole flag operation used by fault injection.
pub trait FaultInjectionFlagReader: Send + Sync {
    fn enabled(&self, id: &str) -> bool;
}

impl FaultInjectionFlagReader for FlagServiceHandle {
    fn enabled(&self, id: &str) -> bool {
        (**self).enabled(id)
    }
}

#[derive(Default)]
struct FaultInjectionState {
    armed: Option<FaultKind>,
    fired: Vec<FaultKind>,
}

pub struct FaultInjectionService {
    flags: Arc<dyn FaultInjectionFlagReader>,
    state: Mutex<FaultInjectionState>,
}

impl FaultInjectionService {
    pub fn new(flags: Arc<dyn FaultInjectionFlagReader>) -> Self {
        Self {
            flags,
            state: Mutex::new(FaultInjectionState::default()),
        }
    }

    pub fn from_flag_service(flags: FlagServiceHandle) -> Self {
        Self::new(Arc::new(flags))
    }
}

impl FaultInjectionServiceContract for FaultInjectionService {
    // Original:
    //   packages/agent-core-v2/src/agent/faultInjection/faultInjectionService.ts
    //   FaultInjectionService.arm()
    fn arm(&self, kind: FaultKind) -> Result<(), Box<Error2>> {
        if !self.flags.enabled(FAULT_INJECTION_FLAG_ID) {
            return Err(Box::new(Error2::new(REQUEST_INVALID, DISABLED_MESSAGE)));
        }
        self.state.lock().unwrap().armed = Some(kind);
        Ok(())
    }

    // Original: FaultInjectionService.status(). Cloning `fired` preserves the
    // original defensive-array snapshot rather than exposing mutable state.
    fn status(&self) -> FaultInjectionStatus {
        let state = self.state.lock().unwrap();
        FaultInjectionStatus {
            armed: state.armed,
            fired: state.fired.clone(),
        }
    }

    // Original: FaultInjectionService.clear().
    fn clear(&self) {
        let mut state = self.state.lock().unwrap();
        state.armed = None;
        state.fired.clear();
    }

    // Original: FaultInjectionService.take(). The state transition is kept in
    // one critical section so concurrent request attempts cannot fire twice.
    fn take(&self) -> Option<FaultKind> {
        let mut state = self.state.lock().unwrap();
        let kind = state.armed.take()?;
        state.fired.push(kind);
        Some(kind)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    struct StubFlags(AtomicBool);

    impl StubFlags {
        fn new(enabled: bool) -> Arc<Self> {
            Arc::new(Self(AtomicBool::new(enabled)))
        }
    }

    impl FaultInjectionFlagReader for StubFlags {
        fn enabled(&self, id: &str) -> bool {
            assert_eq!(id, FAULT_INJECTION_FLAG_ID);
            self.0.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn disabled_arm_preserves_error_code_message_and_state() {
        let service = FaultInjectionService::new(StubFlags::new(false));
        let error = service.arm(FaultKind::ImageFormat).unwrap_err();
        assert_eq!(error.code, REQUEST_INVALID);
        assert_eq!(error.message, DISABLED_MESSAGE);
        assert_eq!(service.status(), FaultInjectionStatus::default());
    }

    #[test]
    fn arm_overwrites_pending_fault_and_take_records_once() {
        let service = FaultInjectionService::new(StubFlags::new(true));
        service.arm(FaultKind::RequestTooLarge).unwrap();
        service.arm(FaultKind::ImageFormat).unwrap();
        assert_eq!(service.status().armed, Some(FaultKind::ImageFormat));

        assert_eq!(service.take(), Some(FaultKind::ImageFormat));
        assert_eq!(service.take(), None);
        assert_eq!(
            service.status(),
            FaultInjectionStatus {
                armed: None,
                fired: vec![FaultKind::ImageFormat],
            }
        );
    }

    #[test]
    fn status_is_defensive_and_clear_resets_both_collections() {
        let service = FaultInjectionService::new(StubFlags::new(true));
        service.arm(FaultKind::RequestTooLarge).unwrap();
        assert_eq!(service.take(), Some(FaultKind::RequestTooLarge));

        let mut snapshot = service.status();
        snapshot.fired.clear();
        assert_eq!(service.status().fired, [FaultKind::RequestTooLarge]);

        service.arm(FaultKind::ImageFormat).unwrap();
        service.clear();
        assert_eq!(service.status(), FaultInjectionStatus::default());
    }

    #[test]
    fn concurrent_take_consumes_an_armed_fault_once() {
        let service = Arc::new(FaultInjectionService::new(StubFlags::new(true)));
        service.arm(FaultKind::RequestTooLarge).unwrap();
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let service = Arc::clone(&service);
                std::thread::spawn(move || service.take())
            })
            .collect();
        let fired: Vec<_> = handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(fired, [FaultKind::RequestTooLarge]);
        assert_eq!(service.status().fired, [FaultKind::RequestTooLarge]);
    }
}
