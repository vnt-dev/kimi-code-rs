//! Media format detection and model-ingestion helpers.
//!
//! Original: `packages/agent-core-v2/src/agent/media`.

pub mod config_section;
pub mod file_type;
pub mod image_compress;
pub mod image_format_policy;
pub mod image_originals;

pub use config_section::*;
pub use file_type::*;
pub use image_compress::*;
pub use image_format_policy::*;
pub use image_originals::*;
