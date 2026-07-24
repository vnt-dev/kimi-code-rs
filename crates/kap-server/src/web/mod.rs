//! Axum HTTP/WebSocket composition for kap-server.
//!
//! Original:
//! `packages/kap-server/src/start.ts`, Fastify construction and middleware.
//!
//! Rust adaptation:
//! Axum owns transport concerns while server-local services remain in their
//! existing modules. Agent-core-v2 calls are isolated behind `core_bridge`.

pub mod core_bridge;
pub mod middleware;
pub mod router;
pub mod state;
pub mod websocket;

pub use core_bridge::{
    AgentCoreBridge, CoreHttpRequest, CoreHttpResponse, CoreOperation, TodoAgentCoreBridge,
};
pub use router::create_router;
pub use state::AppState;
