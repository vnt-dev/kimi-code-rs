use std::sync::LazyLock;

use crate::_base::errors::codes::{ErrorDomain, register_error_domain};

pub const TASK_ID_EMPTY: &str = "task.task_id_empty";

pub static TASK_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[("TASK_ID_EMPTY", TASK_ID_EMPTY)],
    retryable: &[],
    info: &[],
};

static REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&TASK_ERRORS).expect("task error codes are unique");
});

// Original: task/errors.ts, registerErrorDomain(TaskErrors).
pub fn ensure_task_errors_registered() {
    LazyLock::force(&REGISTERED);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::errors::codes::{error_info, is_error_code};

    #[test]
    fn registers_task_id_empty_as_public_non_retryable_error() {
        ensure_task_errors_registered();
        assert!(is_error_code(TASK_ID_EMPTY));
        let info = error_info(TASK_ID_EMPTY);
        assert_eq!(info.title, TASK_ID_EMPTY);
        assert!(!info.retryable);
        assert!(info.public);
        assert!(info.action.is_none());
    }
}
