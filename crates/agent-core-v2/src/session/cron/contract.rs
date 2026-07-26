//! Session-scoped cron scheduler contract.
//!
//! Original: `packages/agent-core-v2/src/session/cron/sessionCronService.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::{
    _base::di::{
        instantiation::ServiceIdentifier,
        lifecycle::{Disposable, DisposeResult},
    },
    agent::loop_::TurnHandle,
    app::cron::{CronTask, CronTaskInit, ParsedCronExpression},
    kosong::contract::message::ContentPart,
};

pub type SessionCronError = Box<dyn Error + Send + Sync>;
pub type SessionCronResult<T> = Result<T, SessionCronError>;
pub type MissedCronRenderer = Arc<dyn Fn(&[CronTask]) -> Vec<ContentPart> + Send + Sync>;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CronLoadOptions {
    pub replace: Option<bool>,
}

#[async_trait]
pub trait SessionCronServiceContract: Disposable + Send + Sync {
    fn is_enabled(&self) -> bool;
    fn is_disabled(&self) -> bool;
    fn add_task(&self, init: CronTaskInit) -> SessionCronResult<CronTask>;
    fn remove_tasks(&self, ids: &[String]) -> SessionCronResult<Vec<String>>;
    fn get_task(&self, id: &str) -> Option<CronTask>;
    fn list(&self) -> Vec<CronTask>;
    fn now(&self) -> f64;
    fn is_stale(&self, task: &CronTask) -> bool;
    fn get_next_fire_time(&self) -> Option<f64>;
    fn get_next_fire_for_task(&self, task_id: &str) -> Option<f64>;
    fn compute_display_next_fire(
        &self,
        task: &CronTask,
        parsed: &ParsedCronExpression,
        ideal_ms: f64,
    ) -> Option<f64>;
    async fn load_from_store(&self, options: CronLoadOptions) -> SessionCronResult<()>;
    async fn start(&self) -> SessionCronResult<()>;
    async fn stop(&self) -> SessionCronResult<()>;
    async fn tick(&self) -> SessionCronResult<()>;
    async fn flush_persist(&self);
    fn handle_missed(
        &self,
        tasks: &[CronTask],
        render_missed_notification: MissedCronRenderer,
    ) -> Option<TurnHandle>;
    fn emit_scheduled(&self, task: &CronTask, agent_id: Option<&str>);
    fn emit_deleted(&self, task_id: &str, agent_id: Option<&str>);
}

#[derive(Clone)]
pub struct SessionCronServiceHandle(pub Arc<dyn SessionCronServiceContract>);

impl Deref for SessionCronServiceHandle {
    type Target = dyn SessionCronServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for SessionCronServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const SESSION_CRON_SERVICE_ID: ServiceIdentifier<SessionCronServiceHandle> =
    ServiceIdentifier::new("sessionCronService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_identity_and_load_defaults_match_source() {
        assert_eq!(SESSION_CRON_SERVICE_ID.to_string(), "sessionCronService");
        assert_eq!(CronLoadOptions::default().replace, None);
    }
}
