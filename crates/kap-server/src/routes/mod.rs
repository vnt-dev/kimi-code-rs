//! HTTP routes grouped to mirror `packages/kap-server/src/routes`.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;

use axum::Json;
use axum::body::to_bytes;
use axum::extract::{FromRequest, Request};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use crate::web::{AppState, CoreHttpRequest, CoreOperation, middleware::RequestId};

pub mod action_suffix;
pub mod approvals;
pub mod auth;
pub mod config;
pub mod connections;
pub mod debug;
pub mod documentation;
pub mod files;
pub mod fs;
pub mod gui_store;
pub mod health;
pub mod messages;
pub mod meta;
pub mod model_catalog;
pub mod oauth;
pub mod prompts;
pub mod questions;
pub mod register_api_v1_routes;
pub mod session_export;
pub mod sessions;
pub mod shutdown;
pub mod skills;
pub mod snapshot;
pub mod tasks;
pub mod terminals;
pub mod tools;
pub mod transcript;
pub mod web_assets;
pub mod workspace_fs;
pub mod workspaces;

#[derive(Debug, Clone, Copy)]
pub struct RouteSpec {
    pub method: &'static str,
    pub runtime_path: &'static str,
    pub document_path: &'static str,
    pub operation: CoreOperation,
}

pub const fn route(
    method: &'static str,
    runtime_path: &'static str,
    document_path: &'static str,
    operation: CoreOperation,
) -> RouteSpec {
    RouteSpec {
        method,
        runtime_path,
        document_path,
        operation,
    }
}

pub fn core_route_specs(debug_endpoints: bool, enable_terminals: bool) -> Vec<RouteSpec> {
    let mut specs = Vec::new();
    for routes in [
        auth::ROUTES,
        oauth::ROUTES,
        config::ROUTES,
        model_catalog::ROUTES,
        sessions::ROUTES,
        session_export::ROUTES,
        skills::ROUTES,
        messages::ROUTES,
        tasks::ROUTES,
        approvals::ROUTES,
        questions::ROUTES,
        prompts::ROUTES,
        workspaces::ROUTES,
        files::ROUTES,
        fs::ROUTES,
        workspace_fs::ROUTES,
        tools::ROUTES,
        snapshot::ROUTES,
        transcript::ROUTES,
    ] {
        specs.extend_from_slice(routes);
    }
    if enable_terminals {
        specs.extend_from_slice(terminals::ROUTES);
    }
    if debug_endpoints {
        specs.extend_from_slice(debug::ROUTES);
    }
    specs
}

pub(crate) struct CoreRouteRequest {
    state: Arc<AppState>,
    request_id: RequestId,
    request: Request,
}

impl FromRequest<Arc<AppState>> for CoreRouteRequest {
    type Rejection = Infallible;

    async fn from_request(
        request: Request,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let request_id = request
            .extensions()
            .get::<RequestId>()
            .cloned()
            .unwrap_or_else(|| RequestId(kimi_code_protocol::parse_or_generate_request_id(None)));
        Ok(Self {
            state: Arc::clone(state),
            request_id,
            request,
        })
    }
}

pub(crate) async fn dispatch_core(
    operation: CoreOperation,
    route_request: CoreRouteRequest,
) -> Response {
    let CoreRouteRequest {
        state,
        request_id,
        request,
    } = route_request;
    let method = request.method().to_string();
    let path = request.uri().path().to_owned();
    let query = request.uri().query().map(str::to_owned);
    let headers = request
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.to_string(), value.to_owned()))
        })
        .collect::<BTreeMap<_, _>>();
    let body = match to_bytes(request.into_body(), usize::MAX).await {
        Ok(body) => body.to_vec(),
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "code": 40000,
                    "msg": error.to_string(),
                    "data": null,
                    "request_id": request_id.0
                })),
            )
                .into_response();
        }
    };
    let response = state
        .core_bridge
        .invoke(
            operation,
            CoreHttpRequest {
                method,
                path,
                query,
                headers,
                body,
            },
        )
        .await;
    let mut result = (
        StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(response.body),
    )
        .into_response();
    for (name, value) in response.headers {
        if let (Ok(name), Ok(value)) = (HeaderName::try_from(name), HeaderValue::try_from(value)) {
            result.headers_mut().insert(name, value);
        }
    }
    result
}

pub fn create_open_api_document(state: &AppState) -> Value {
    let mut paths = Map::new();
    for (method, path) in documented_route_pairs(state) {
        paths
            .entry(path)
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .expect("OpenAPI path entry is always an object")
            .insert(
                method.to_ascii_lowercase(),
                json!({"responses": {"200": {"description": "kap-server response"}}}),
            );
    }
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Kimi Code Server API",
            "description": "REST API for the Kimi Code local server. All JSON responses are wrapped in a uniform envelope `{ code, msg, data, request_id }`.",
            "version": state.server_version
        },
        "paths": paths
    })
}

pub fn documented_route_pairs(state: &AppState) -> Vec<(String, String)> {
    documented_route_pairs_for(
        state.debug_endpoints,
        state.enable_terminals,
        state.enable_shutdown,
    )
}

fn documented_route_pairs_for(
    debug_endpoints: bool,
    enable_terminals: bool,
    enable_shutdown: bool,
) -> Vec<(String, String)> {
    let mut pairs = core_route_specs(debug_endpoints, enable_terminals)
        .into_iter()
        .map(|route| (route.method.to_owned(), route.document_path.to_owned()))
        .collect::<Vec<_>>();
    pairs.extend([
        ("GET".into(), "/api/v1/healthz".into()),
        ("GET".into(), "/api/v1/meta".into()),
        ("GET".into(), "/api/v1/connections".into()),
        ("GET".into(), "/api/v1/gui/store/getItem".into()),
        ("GET".into(), "/api/v1/gui/store/length".into()),
        ("POST".into(), "/api/v1/gui/store/clear".into()),
        ("POST".into(), "/api/v1/gui/store/removeItem".into()),
        ("POST".into(), "/api/v1/gui/store/setItem".into()),
        ("GET".into(), "/asyncapi.json".into()),
        ("GET".into(), "/openapi.json".into()),
    ]);
    if enable_shutdown {
        pairs.push(("POST".into(), "/api/v1/shutdown".into()));
    }
    pairs.sort();
    pairs.dedup();
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_surface_matches_typescript_snapshot() {
        let mut expected = include_str!("../web/api_surface.txt")
            .lines()
            .map(|line| {
                let (method, path) = line.split_once(' ').unwrap();
                (method.to_owned(), path.to_owned())
            })
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(documented_route_pairs_for(true, true, true), expected);
    }

    #[test]
    fn production_gates_debug_terminal_and_shutdown_interfaces() {
        let routes = documented_route_pairs_for(false, false, false);
        assert!(
            routes
                .iter()
                .all(|(_, path)| !path.starts_with("/api/v1/debug/"))
        );
        assert!(routes.iter().all(|(_, path)| !path.contains("/terminals")));
        assert!(!routes.contains(&("POST".into(), "/api/v1/shutdown".into())));
    }
}
