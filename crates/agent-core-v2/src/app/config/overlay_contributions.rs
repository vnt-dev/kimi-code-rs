//! Module-level effective-config overlay contribution collector.
//!
//! Original: `packages/agent-core-v2/src/app/config/configOverlayContributions.ts`.

use parking_lot::RwLock;
use std::sync::{Arc, LazyLock};

use super::contract::ConfigEffectiveOverlay;

static OVERLAYS: LazyLock<RwLock<Vec<Arc<dyn ConfigEffectiveOverlay>>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

// Original: registerConfigOverlay().
pub fn register_config_overlay(overlay: Arc<dyn ConfigEffectiveOverlay>) {
    OVERLAYS.write().push(overlay);
}

// Original: getConfigOverlayContributions().
pub fn get_config_overlay_contributions() -> Vec<Arc<dyn ConfigEffectiveOverlay>> {
    OVERLAYS.read().clone()
}

// Original: _clearConfigOverlayContributionsForTests().
pub fn clear_config_overlay_contributions_for_tests() {
    OVERLAYS.write().clear();
}
