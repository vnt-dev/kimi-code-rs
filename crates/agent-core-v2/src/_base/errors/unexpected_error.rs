use std::{
    error::Error,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, LazyLock, RwLock},
};

type UnexpectedErrorHandler = Arc<dyn Fn(&(dyn Error + 'static)) + Send + Sync>;

fn default_handler(error: &(dyn Error + 'static)) {
    eprintln!("[unexpected] {error}");
}

static CURRENT_HANDLER: LazyLock<RwLock<UnexpectedErrorHandler>> =
    LazyLock::new(|| RwLock::new(Arc::new(default_handler)));

pub fn set_unexpected_error_handler(
    handler: impl Fn(&(dyn Error + 'static)) + Send + Sync + 'static,
) {
    *CURRENT_HANDLER.write().unwrap() = Arc::new(handler);
}

pub fn reset_unexpected_error_handler() {
    *CURRENT_HANDLER.write().unwrap() = Arc::new(default_handler);
}

// Original: packages/agent-core-v2/src/_base/errors/unexpectedError.ts,
// onUnexpectedError(). A panicking reporting hook is contained like a thrown JS hook.
pub fn on_unexpected_error(error: &(dyn Error + 'static)) {
    let handler = Arc::clone(&CURRENT_HANDLER.read().unwrap());
    if catch_unwind(AssertUnwindSafe(|| handler(error))).is_err() {
        eprintln!("[unexpected] handler panicked while reporting {error}");
    }
}

pub fn safely_call_listener(listener: impl FnOnce()) {
    if let Err(payload) = catch_unwind(AssertUnwindSafe(listener)) {
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("listener panicked");
        on_unexpected_error(&std::io::Error::other(message));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn replaces_resets_and_contains_panicking_handlers() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&seen);
        set_unexpected_error_handler(move |error| {
            captured.lock().unwrap().push(error.to_string());
        });
        safely_call_listener(|| panic!("listener-boom"));
        assert_eq!(*seen.lock().unwrap(), vec!["listener-boom"]);

        set_unexpected_error_handler(|_| panic!("handler-boom"));
        on_unexpected_error(&std::io::Error::other("original"));
        reset_unexpected_error_handler();
    }
}
