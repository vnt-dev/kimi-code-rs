use std::{ops::Deref, sync::Arc};

use crate::_base::{di::instantiation::ServiceIdentifier, errors::errors::Error2};

/// A deterministic provider failure that can be consumed once.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FaultKind {
    RequestTooLarge,
    ImageFormat,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FaultInjectionStatus {
    pub armed: Option<FaultKind>,
    pub fired: Vec<FaultKind>,
}

pub trait FaultInjectionServiceContract: Send + Sync {
    fn arm(&self, kind: FaultKind) -> Result<(), Box<Error2>>;
    fn status(&self) -> FaultInjectionStatus;
    fn clear(&self);
    fn take(&self) -> Option<FaultKind>;
}

#[derive(Clone)]
pub struct FaultInjectionServiceHandle(pub Arc<dyn FaultInjectionServiceContract>);

impl Deref for FaultInjectionServiceHandle {
    type Target = dyn FaultInjectionServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

// Original:
//   packages/agent-core-v2/src/agent/faultInjection/faultInjection.ts
//   IFaultInjectionService
pub const FAULT_INJECTION_SERVICE_ID: ServiceIdentifier<FaultInjectionServiceHandle> =
    ServiceIdentifier::new("faultInjectionService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fault_kind_preserves_wire_spellings() {
        assert_eq!(
            serde_json::to_string(&FaultKind::RequestTooLarge).unwrap(),
            r#""request-too-large""#
        );
        assert_eq!(
            serde_json::from_str::<FaultKind>(r#""image-format""#).unwrap(),
            FaultKind::ImageFormat
        );
        assert_eq!(
            FAULT_INJECTION_SERVICE_ID.to_string(),
            "faultInjectionService"
        );
    }
}
