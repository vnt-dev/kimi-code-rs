//! Layered application configuration.

pub mod contract;
pub mod pure;
pub mod section_diff;
pub mod toml;

pub use contract::*;
pub use pure::{deep_equal, deep_merge, is_plain_object, omit_undefined};
pub use section_diff::{RecordDiff, diff_records};
pub use toml::{
    camel_to_snake, clone_record, plain_object_to_toml, set_defined, snake_to_camel,
    transform_plain_object,
};
