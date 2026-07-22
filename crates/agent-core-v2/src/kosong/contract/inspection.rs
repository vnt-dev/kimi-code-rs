use serde::{Deserialize, Serialize};
use std::any::Any;
use std::sync::Arc;

// Original:
//   packages/agent-core-v2/src/kosong/contract/inspection.ts
//   InspectionSourceKind
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InspectionSourceKind {
    Config,
    Override,
    Builtin,
    Env,
    Synthesized,
    None,
}

// Original: inspection.ts, InspectionSource
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectionSource {
    pub kind: InspectionSourceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

pub type CapturedResolutionValue = Arc<dyn Any + Send + Sync>;

// Original:
//   packages/agent-core-v2/src/kosong/contract/inspection.ts
//   ResolutionTrace
//
// Rust adaptation:
//   `unknown` capture values become shared type-erased references. They stay
//   reference-only and can be downcast by the eventual inspector without
//   requiring serialization, cloning, or premature secret redaction.
pub trait ResolutionTrace {
    fn record(&mut self, path: &str, source: InspectionSource);
    fn capture(&mut self, key: &str, value: CapturedResolutionValue);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Trace {
        sources: HashMap<String, InspectionSource>,
        captures: HashMap<String, CapturedResolutionValue>,
    }

    impl ResolutionTrace for Trace {
        fn record(&mut self, path: &str, source: InspectionSource) {
            self.sources.insert(path.to_owned(), source);
        }

        fn capture(&mut self, key: &str, value: CapturedResolutionValue) {
            self.captures.insert(key.to_owned(), value);
        }
    }

    #[test]
    fn source_serialization_preserves_contract_vocabulary() {
        assert_eq!(
            serde_json::to_value(InspectionSource {
                kind: InspectionSourceKind::Env,
                detail: Some("KIMI_API_KEY (provider env bag)".to_owned()),
            })
            .unwrap(),
            serde_json::json!({
                "kind": "env",
                "detail": "KIMI_API_KEY (provider env bag)",
            })
        );
        assert_eq!(
            serde_json::to_value(InspectionSource {
                kind: InspectionSourceKind::None,
                detail: None,
            })
            .unwrap(),
            serde_json::json!({"kind": "none"})
        );
    }

    #[test]
    fn trace_records_sources_and_reference_only_typed_captures() {
        let mut trace = Trace::default();
        trace.record(
            "resolved.auth",
            InspectionSource {
                kind: InspectionSourceKind::Config,
                detail: Some("model.apiKey".to_owned()),
            },
        );
        trace.capture("provider", Arc::new(vec!["kimi".to_owned()]));

        assert_eq!(
            trace.sources["resolved.auth"].kind,
            InspectionSourceKind::Config
        );
        assert_eq!(
            trace.captures["provider"]
                .downcast_ref::<Vec<String>>()
                .unwrap(),
            &["kimi".to_owned()]
        );
    }
}
