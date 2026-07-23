//! Layered application configuration.

pub mod contract;
pub mod overlay_contributions;
pub mod pure;
pub mod registry_service;
pub mod section_contributions;
pub mod section_diff;
pub mod toml;

pub use contract::*;
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
pub use toml::{
    camel_to_snake, clone_record, plain_object_to_toml, set_defined, snake_to_camel,
    transform_plain_object,
};
