mod agent_rpc;
mod app_events;
mod controller;
mod plugin_marketplace;
mod rpc;
mod server;
mod settings;
mod wire;

pub use agent_rpc::{AgentRpcMethod, AgentRpcRequest, RpcError, dispatch_agent_rpc};
pub use app_events::{ApplicationEventHandler, DESKTOP_STATE_CHANGED_EVENT, DesktopStateChange};
pub use controller::{
    DEFAULT_WEB_SERVER_PORT, WebServerController, WebServerState, WebServerStatus,
};
pub use plugin_marketplace::{PluginMarketplace, PluginMarketplaceEntry, load_plugin_marketplace};
pub use server::{AssetProvider, WebAsset};
pub use settings::{WebServerListenScope, WebServerSettings};
pub use wire::{RpcRequest, RpcResponse, RpcResponseError};
