//! Cron scheduler foundations.
//!
//! Original: `packages/agent-core-v2/src/app/cron`.

pub mod clock;
pub mod config_section;
pub mod cron_task;
pub mod cron_task_persistence;

pub use clock::{ClockSources, SYSTEM_CLOCKS, resolve_clock_sources};
pub use config_section::{CRON_SECTION, DEFAULT_CRON_CONFIG, register_cron_config_section};
pub use cron_task::{CRON_SESSION_TAG, CronTask, CronTaskInit};
pub use cron_task_persistence::{
    CRON_TASK_PERSISTENCE_SERVICE_ID, CronTaskPersistenceContract, CronTaskPersistenceHandle,
    CronTaskPersistenceResult, CronTaskQuery,
};
