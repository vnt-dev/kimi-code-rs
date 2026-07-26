#[path = "wire.rs"]
pub mod contract;
pub mod errors;
pub mod migration;
pub mod model;
pub mod op;
pub mod record;
pub mod wire_service;

pub use wire_service::register_wire_service;
