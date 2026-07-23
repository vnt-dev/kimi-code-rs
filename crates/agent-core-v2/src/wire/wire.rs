//! Agent-scoped replayable wire aggregate contract.
//!
//! Original: `packages/agent-core-v2/src/wire/wire.ts` and the
//! `CycleError` declaration in `wire/wireService.ts`.

use std::{error::Error, fmt, ops::Deref, sync::Arc};

use serde_json::{Map, Value};

use crate::{
    _base::{di::instantiation::ServiceIdentifier, errors::errors::Error2Options},
    hooks::OrderedHookSlot,
};

use super::errors::{WIRE_CYCLE, WireError};
use super::wire_service::WireService;

#[derive(Clone)]
pub struct WireServiceHandle(pub Arc<WireService>);

impl Deref for WireServiceHandle {
    type Target = WireService;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const WIRE_SERVICE_ID: ServiceIdentifier<WireServiceHandle> =
    ServiceIdentifier::new("wireService");

pub const MAX_DRAIN: usize = 100;

#[derive(Default)]
pub struct WireHooks {
    pub on_did_restore: OrderedHookSlot<()>,
}

#[derive(Debug)]
pub struct CycleError {
    pub depth: usize,
    pub op_types: Vec<String>,
    inner: WireError,
}

impl CycleError {
    pub fn new(depth: usize, op_types: impl IntoIterator<Item = String>) -> Self {
        let op_types = op_types.into_iter().take(20).collect::<Vec<_>>();
        let details = Map::from_iter([
            ("depth".into(), Value::from(depth as u64)),
            (
                "opTypes".into(),
                Value::Array(op_types.iter().cloned().map(Value::String).collect()),
            ),
        ]);
        let inner = WireError::with_options(
            WIRE_CYCLE,
            format!("Wire dispatch cascade exceeded MAX_DRAIN ({depth}); possible op cycle"),
            Error2Options {
                details: Some(details),
                name: Some("CycleError".into()),
                ..Error2Options::default()
            },
        );
        Self {
            depth,
            op_types,
            inner,
        }
    }

    pub fn error(&self) -> &WireError {
        &self.inner
    }
}

impl fmt::Display for CycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(formatter)
    }
}

impl Error for CycleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_error_preserves_depth_and_limits_reported_op_types() {
        let error = CycleError::new(101, (0..25).map(|index| format!("op.{index}")));
        assert_eq!(error.depth, 101);
        assert_eq!(error.op_types.len(), 20);
        assert_eq!(error.error().code(), WIRE_CYCLE);
        assert_eq!(error.error().error().name, "CycleError");
        assert_eq!(
            error.error().error().details.as_ref().unwrap()["depth"],
            101
        );
    }

    #[test]
    fn service_identifier_matches_source() {
        assert_eq!(WIRE_SERVICE_ID.to_string(), "wireService");
    }
}
