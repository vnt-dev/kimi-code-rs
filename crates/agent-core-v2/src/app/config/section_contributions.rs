//! Module-level config-section contribution collector.
//!
//! Original: `packages/agent-core-v2/src/app/config/configSectionContributions.ts`.

use std::sync::{LazyLock, RwLock};

use super::contract::{ConfigSchema, RegisterSectionOptions};

#[derive(Clone)]
pub struct ConfigSectionContribution {
    pub domain: String,
    pub schema: ConfigSchema,
    pub options: RegisterSectionOptions,
}

static CONTRIBUTIONS: LazyLock<RwLock<Vec<ConfigSectionContribution>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

// Original: registerConfigSection().
pub fn register_config_section(
    domain: impl Into<String>,
    schema: ConfigSchema,
    options: RegisterSectionOptions,
) {
    CONTRIBUTIONS
        .write()
        .unwrap()
        .push(ConfigSectionContribution {
            domain: domain.into(),
            schema,
            options,
        });
}

// Original: getConfigSectionContributions().
pub fn get_config_section_contributions() -> Vec<ConfigSectionContribution> {
    CONTRIBUTIONS.read().unwrap().clone()
}

// Original: _clearConfigSectionContributionsForTests().
pub fn clear_config_section_contributions_for_tests() {
    CONTRIBUTIONS.write().unwrap().clear();
}
