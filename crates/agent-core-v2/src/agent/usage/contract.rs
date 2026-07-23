use std::{fmt, ops::Deref, sync::Arc};

use serde_json::Map;

use crate::{
    _base::{
        di::{instantiation::ServiceIdentifier, lifecycle::Disposable},
        errors::errors::{Error2, Error2Options},
        event::Event,
    },
    agent::llm_requester::AgentLlmRequestSource,
    kosong::contract::usage::TokenUsage,
};

use super::{
    USAGE_TURN_ID_CONFLICT, UsageServiceError, UsageStatus, ensure_usage_errors_registered,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsageErrorCode {
    TurnIdConflict,
}

impl UsageErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TurnIdConflict => USAGE_TURN_ID_CONFLICT,
        }
    }
}

#[derive(Debug)]
pub struct UsageError(Error2);

impl UsageError {
    pub fn new(
        code: UsageErrorCode,
        message: impl Into<String>,
        details: Option<Map<String, serde_json::Value>>,
    ) -> Self {
        ensure_usage_errors_registered();
        Self(Error2::with_options(
            code.as_str(),
            message,
            Error2Options {
                details,
                name: Some("UsageError".into()),
                ..Error2Options::default()
            },
        ))
    }

    pub fn error(&self) -> &Error2 {
        &self.0
    }
}

impl fmt::Display for UsageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for UsageError {}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageRecordedContext {
    pub model: String,
    pub usage: TokenUsage,
    pub source: Option<AgentLlmRequestSource>,
}

pub trait AgentUsageServiceContract: Disposable + Send + Sync {
    fn record(
        &self,
        model: String,
        usage: TokenUsage,
        source: Option<AgentLlmRequestSource>,
    ) -> Result<(), UsageServiceError>;
    fn status(&self) -> UsageStatus;
    fn on_did_record(&self) -> Event<UsageRecordedContext>;
}

#[derive(Clone)]
pub struct AgentUsageServiceHandle(pub Arc<dyn AgentUsageServiceContract>);

impl Deref for AgentUsageServiceHandle {
    type Target = dyn AgentUsageServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentUsageServiceHandle {
    fn dispose(&self) -> crate::_base::di::lifecycle::DisposeResult {
        self.0.dispose()
    }
}

pub const AGENT_USAGE_SERVICE_ID: ServiceIdentifier<AgentUsageServiceHandle> =
    ServiceIdentifier::new("agentUsageService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_error_and_service_identifier_match_source() {
        let details = Map::from_iter([("turnId".into(), serde_json::Value::from(2))]);
        let error = UsageError::new(
            UsageErrorCode::TurnIdConflict,
            "turn changed",
            Some(details),
        );
        assert_eq!(error.error().name, "UsageError");
        assert_eq!(error.error().code, USAGE_TURN_ID_CONFLICT);
        assert_eq!(error.error().details.as_ref().unwrap()["turnId"], 2);
        assert_eq!(AGENT_USAGE_SERVICE_ID.to_string(), "agentUsageService");
    }
}
