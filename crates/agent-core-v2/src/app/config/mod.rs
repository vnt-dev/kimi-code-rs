//! Layered application configuration.

pub mod contract;
pub mod env;
pub mod migrations;
pub mod overlay_contributions;
pub mod pure;
pub mod registry_service;
pub mod section_contributions;
pub mod section_diff;
pub mod service;
pub mod toml;

pub use contract::*;
pub use env::apply_section_env;
pub use migrations::migrate_thinking_effort_max_to_high;
pub use overlay_contributions::{
    clear_config_overlay_contributions_for_tests, get_config_overlay_contributions,
    register_config_overlay,
};
pub use pure::{deep_equal, deep_merge, is_plain_object, omit_undefined};
pub use registry_service::{ConfigRegistry, register_config_registry};
pub use section_contributions::{
    ConfigSectionContribution, clear_config_section_contributions_for_tests,
    get_config_section_contributions, register_config_section,
};
pub use section_diff::{RecordDiff, diff_records};
pub use service::{ConfigService, register_config_service};
pub use toml::{
    apply_section_to_toml, camel_to_snake, clone_record, plain_object_to_toml, set_defined,
    snake_to_camel, transform_plain_object, transform_toml_data,
};
