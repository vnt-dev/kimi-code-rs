//! Provider-discovery service helpers.
//!
//! Original: `packages/agent-core-v2/src/kosong/model/discoveryService.ts`,
//! `withoutKeys()` and `mapRefreshResult()`.

use indexmap::IndexMap;
use kimi_code_oauth::RefreshResult;

use super::discovery::{
    ProviderRefreshChange, ProviderRefreshFailure, RefreshProviderModelsResponse,
};

// Original: discoveryService.ts, withoutKeys().
// IndexMap preserves the source object's configured insertion order.
pub fn without_keys<T: Clone>(
    record: &IndexMap<String, T>,
    excluded: &IndexMap<String, impl Sized>,
) -> IndexMap<String, T> {
    record
        .iter()
        .filter(|(key, _)| !excluded.contains_key(*key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

// Original: discoveryService.ts, mapRefreshResult().
pub fn map_refresh_result(result: RefreshResult) -> RefreshProviderModelsResponse {
    RefreshProviderModelsResponse {
        changed: result
            .changed
            .into_iter()
            .map(|change| ProviderRefreshChange {
                provider_id: change.provider_id,
                provider_name: change.provider_name,
                added: change.added as u64,
                removed: change.removed as u64,
            })
            .collect(),
        unchanged: result.unchanged,
        failed: result
            .failed
            .into_iter()
            .map(|failure| ProviderRefreshFailure {
                provider: failure.provider,
                reason: failure.reason,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use kimi_code_oauth::{ProviderChange, ProviderRefreshFailure};

    use super::*;

    #[test]
    fn removes_only_excluded_records_and_maps_refresh_wire_fields() {
        let retained = without_keys(
            &IndexMap::from([("static".into(), 1), ("refreshable".into(), 2)]),
            &IndexMap::from([("static".into(), ())]),
        );
        assert_eq!(retained, IndexMap::from([("refreshable".into(), 2)]));

        let response = map_refresh_result(RefreshResult {
            changed: vec![ProviderChange {
                provider_id: "kimi".into(),
                provider_name: "Kimi".into(),
                added: 2,
                removed: 1,
            }],
            unchanged: vec!["static".into()],
            failed: vec![ProviderRefreshFailure {
                provider: "openai".into(),
                reason: "offline".into(),
            }],
        });
        assert_eq!(response.changed[0].provider_id, "kimi");
        assert_eq!(response.changed[0].added, 2);
        assert_eq!(response.unchanged, ["static"]);
        assert_eq!(response.failed[0].reason, "offline");
    }
}
