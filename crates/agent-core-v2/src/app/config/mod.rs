//! Layered application configuration.

pub mod pure;
pub mod section_diff;

pub use pure::{deep_equal, deep_merge, is_plain_object, omit_undefined};
pub use section_diff::{RecordDiff, diff_records};
