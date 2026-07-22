pub mod adapter;
pub mod controller;

pub use adapter::{adapt_approval_request, adapt_panel_response};
pub use controller::ApprovalController;
