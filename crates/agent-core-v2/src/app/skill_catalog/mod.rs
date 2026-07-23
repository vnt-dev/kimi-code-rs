//! Skill discovery, parsing, and catalog services.

pub mod parser;

pub use parser::{FrontmatterError, ParsedFrontmatter, parse_frontmatter};
