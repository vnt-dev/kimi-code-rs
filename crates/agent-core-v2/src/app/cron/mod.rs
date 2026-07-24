//! Cron scheduler foundations.
//!
//! Original: `packages/agent-core-v2/src/app/cron`.

pub mod clock;
pub mod config_section;
pub mod cron_expr;
pub mod cron_task;
pub mod cron_task_persistence;
pub mod cron_task_persistence_service;
pub mod format;
pub mod jitter;

pub use clock::{ClockSources, SYSTEM_CLOCKS, resolve_clock_sources};
pub use config_section::{CRON_SECTION, DEFAULT_CRON_CONFIG, register_cron_config_section};
pub use cron_expr::{
    CronExpressionError, ParsedCronExpression, compute_next_cron_run, cron_to_human,
    has_fire_within_years, parse_cron_expression,
};
pub use cron_task::{CRON_SESSION_TAG, CronTask, CronTaskInit};
pub use cron_task_persistence::{
    CRON_TASK_PERSISTENCE_SERVICE_ID, CronTaskPersistenceContract, CronTaskPersistenceHandle,
    CronTaskPersistenceResult, CronTaskQuery,
};
pub use cron_task_persistence_service::{
    CRON_ID_REGEX, CronTaskPersistenceService, is_valid_cron_task,
    register_cron_task_persistence_service,
};
pub use format::{format_local_iso_with_offset, render_cron_fire_xml};
pub use jitter::{
    DEFAULT_CRON_JITTER_CONFIG, JitterConfig, jittered_next_cron_run_ms,
    one_shot_jittered_next_cron_run_ms,
};
