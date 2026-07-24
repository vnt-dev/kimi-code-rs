use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{Extension, Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodFilter, on};
use axum::{Json, Router};
use serde_json::{Map, Value, json};

use super::core_bridge::{CoreHttpRequest, CoreOperation};
use super::middleware::RequestId;
use super::state::AppState;

#[derive(Debug, Clone, Copy)]
pub struct RouteSpec {
    pub method: &'static str,
    pub runtime_path: &'static str,
    pub document_path: &'static str,
    pub operation: CoreOperation,
}

const CORE_ROUTES: &[RouteSpec] = &[
    spec(
        "GET",
        "/api/v1/auth",
        "/api/v1/auth",
        CoreOperation::GetAuth,
    ),
    spec(
        "GET",
        "/api/v1/config",
        "/api/v1/config",
        CoreOperation::GetConfig,
    ),
    spec(
        "POST",
        "/api/v1/config",
        "/api/v1/config",
        CoreOperation::UpdateConfig,
    ),
    spec(
        "GET",
        "/api/v1/oauth/login",
        "/api/v1/oauth/login",
        CoreOperation::GetOauthLogin,
    ),
    spec(
        "POST",
        "/api/v1/oauth/login",
        "/api/v1/oauth/login",
        CoreOperation::StartOauthLogin,
    ),
    spec(
        "DELETE",
        "/api/v1/oauth/login",
        "/api/v1/oauth/login",
        CoreOperation::DeleteOauthLogin,
    ),
    spec(
        "POST",
        "/api/v1/oauth/logout",
        "/api/v1/oauth/logout",
        CoreOperation::OauthLogout,
    ),
    spec(
        "GET",
        "/api/v1/models",
        "/api/v1/models",
        CoreOperation::ListModels,
    ),
    spec(
        "POST",
        "/api/v1/models/{tail}",
        "/api/v1/models/{tail}",
        CoreOperation::ModelAction,
    ),
    spec(
        "GET",
        "/api/v1/providers",
        "/api/v1/providers",
        CoreOperation::ListProviders,
    ),
    spec(
        "GET",
        "/api/v1/providers/{item}",
        "/api/v1/providers/{provider_id}",
        CoreOperation::GetProvider,
    ),
    spec(
        "POST",
        "/api/v1/providers:refresh",
        "/api/v1/providers{action}",
        CoreOperation::ProviderCollectionAction,
    ),
    spec(
        "POST",
        "/api/v1/providers:refresh_oauth",
        "/api/v1/providers{action}",
        CoreOperation::ProviderCollectionAction,
    ),
    spec(
        "POST",
        "/api/v1/providers/{item}",
        "/api/v1/providers/{tail}",
        CoreOperation::ProviderAction,
    ),
    spec(
        "GET",
        "/api/v1/sessions",
        "/api/v1/sessions",
        CoreOperation::ListSessions,
    ),
    spec(
        "POST",
        "/api/v1/sessions",
        "/api/v1/sessions",
        CoreOperation::CreateSession,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_ref}",
        "/api/v1/sessions/{session_id}",
        CoreOperation::GetSession,
    ),
    spec(
        "POST",
        "/api/v1/sessions/{session_ref}",
        "/api/v1/sessions/{session_id}:archive",
        CoreOperation::SessionAction,
    ),
    spec(
        "POST",
        "/api/v1/sessions/{session_id}/{tail}",
        "/api/v1/sessions/{session_id}/{tail}",
        CoreOperation::SessionNestedAction,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/children",
        "/api/v1/sessions/{session_id}/children",
        CoreOperation::ListSessionChildren,
    ),
    spec(
        "POST",
        "/api/v1/sessions/{session_id}/children",
        "/api/v1/sessions/{session_id}/children",
        CoreOperation::CreateSessionChild,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/goal",
        "/api/v1/sessions/{session_id}/goal",
        CoreOperation::GetSessionGoal,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/profile",
        "/api/v1/sessions/{session_id}/profile",
        CoreOperation::GetSessionProfile,
    ),
    spec(
        "POST",
        "/api/v1/sessions/{session_id}/profile",
        "/api/v1/sessions/{session_id}/profile",
        CoreOperation::UpdateSessionProfile,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/status",
        "/api/v1/sessions/{session_id}/status",
        CoreOperation::GetSessionStatus,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/warnings",
        "/api/v1/sessions/{session_id}/warnings",
        CoreOperation::GetSessionWarnings,
    ),
    spec(
        "POST",
        "/api/v1/sessions/{session_id}/export",
        "/api/v1/sessions/{session_id}/export",
        CoreOperation::ExportSession,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/messages",
        "/api/v1/sessions/{session_id}/messages",
        CoreOperation::ListMessages,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/messages/{message_id}",
        "/api/v1/sessions/{session_id}/messages/{message_id}",
        CoreOperation::GetMessage,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/prompts",
        "/api/v1/sessions/{session_id}/prompts",
        CoreOperation::ListPrompts,
    ),
    spec(
        "POST",
        "/api/v1/sessions/{session_id}/prompts",
        "/api/v1/sessions/{session_id}/prompts",
        CoreOperation::SubmitPrompt,
    ),
    spec(
        "POST",
        "/api/v1/sessions/{session_id}/prompts:steer",
        "/api/v1/sessions/{session_id}/prompts:steer",
        CoreOperation::SteerPrompt,
    ),
    spec(
        "POST",
        "/api/v1/sessions/{session_id}/prompts/{tail}",
        "/api/v1/sessions/{session_id}/prompts/{tail}",
        CoreOperation::PromptAction,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/approvals",
        "/api/v1/sessions/{session_id}/approvals",
        CoreOperation::ListApprovals,
    ),
    spec(
        "POST",
        "/api/v1/sessions/{session_id}/approvals/{approval_id}",
        "/api/v1/sessions/{session_id}/approvals/{approval_id}",
        CoreOperation::ResolveApproval,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/questions",
        "/api/v1/sessions/{session_id}/questions",
        CoreOperation::ListQuestions,
    ),
    spec(
        "POST",
        "/api/v1/sessions/{session_id}/questions/{tail}",
        "/api/v1/sessions/{session_id}/questions/{tail}",
        CoreOperation::QuestionAction,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/skills",
        "/api/v1/sessions/{session_id}/skills",
        CoreOperation::ListSkills,
    ),
    spec(
        "POST",
        "/api/v1/sessions/{session_id}/skills/{tail}",
        "/api/v1/sessions/{session_id}/skills/{tail}",
        CoreOperation::SkillAction,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/tasks",
        "/api/v1/sessions/{session_id}/tasks",
        CoreOperation::ListTasks,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/tasks/{item}",
        "/api/v1/sessions/{session_id}/tasks/{task_id}",
        CoreOperation::GetTask,
    ),
    spec(
        "POST",
        "/api/v1/sessions/{session_id}/tasks/{item}",
        "/api/v1/sessions/{session_id}/tasks/{tail}",
        CoreOperation::TaskAction,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/transcript",
        "/api/v1/sessions/{session_id}/transcript",
        CoreOperation::GetTranscript,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/snapshot",
        "/api/v1/sessions/{session_id}/snapshot",
        CoreOperation::GetSnapshot,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/fs/{*path}",
        "/api/v1/sessions/{session_id}/fs/{*}",
        CoreOperation::ReadSessionFile,
    ),
    spec(
        "GET",
        "/api/v1/workspaces",
        "/api/v1/workspaces",
        CoreOperation::ListWorkspaces,
    ),
    spec(
        "POST",
        "/api/v1/workspaces",
        "/api/v1/workspaces",
        CoreOperation::CreateWorkspace,
    ),
    spec(
        "PATCH",
        "/api/v1/workspaces/{workspace_id}",
        "/api/v1/workspaces/{workspace_id}",
        CoreOperation::UpdateWorkspace,
    ),
    spec(
        "DELETE",
        "/api/v1/workspaces/{workspace_id}",
        "/api/v1/workspaces/{workspace_id}",
        CoreOperation::DeleteWorkspace,
    ),
    spec(
        "GET",
        "/api/v1/workspaces/{workspace_id}/skills",
        "/api/v1/workspaces/{workspace_id}/skills",
        CoreOperation::ListWorkspaceSkills,
    ),
    spec(
        "GET",
        "/api/v1/fs:browse",
        "/api/v1/fs:browse",
        CoreOperation::BrowseFileSystem,
    ),
    spec(
        "GET",
        "/api/v1/fs:home",
        "/api/v1/fs:home",
        CoreOperation::GetFileSystemHome,
    ),
    spec(
        "POST",
        "/api/v1/files",
        "/api/v1/files",
        CoreOperation::UploadFile,
    ),
    spec(
        "GET",
        "/api/v1/files/{file_id}",
        "/api/v1/files/{file_id}",
        CoreOperation::DownloadFile,
    ),
    spec(
        "DELETE",
        "/api/v1/files/{file_id}",
        "/api/v1/files/{file_id}",
        CoreOperation::DeleteFile,
    ),
    spec(
        "GET",
        "/api/v1/tools",
        "/api/v1/tools",
        CoreOperation::ListTools,
    ),
    spec(
        "GET",
        "/api/v1/mcp/servers",
        "/api/v1/mcp/servers",
        CoreOperation::ListMcpServers,
    ),
    spec(
        "POST",
        "/api/v1/mcp/servers/{tail}",
        "/api/v1/mcp/servers/{tail}",
        CoreOperation::McpServerAction,
    ),
];

const TERMINAL_ROUTES: &[RouteSpec] = &[
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/terminals",
        "/api/v1/sessions/{session_id}/terminals",
        CoreOperation::ListTerminals,
    ),
    spec(
        "POST",
        "/api/v1/sessions/{session_id}/terminals",
        "/api/v1/sessions/{session_id}/terminals",
        CoreOperation::CreateTerminal,
    ),
    spec(
        "GET",
        "/api/v1/sessions/{session_id}/terminals/{item}",
        "/api/v1/sessions/{session_id}/terminals/{terminal_id}",
        CoreOperation::GetTerminal,
    ),
    spec(
        "POST",
        "/api/v1/sessions/{session_id}/terminals/{item}",
        "/api/v1/sessions/{session_id}/terminals/{tail}",
        CoreOperation::TerminalAction,
    ),
];

const DEBUG_ROUTES: &[RouteSpec] = &[
    spec(
        "GET",
        "/api/v1/debug/channels",
        "/api/v1/debug/channels",
        CoreOperation::DebugChannels,
    ),
    spec(
        "GET",
        "/api/v1/debug/{service}/{method}",
        "/api/v1/debug/{service}/{method}",
        CoreOperation::DebugGlobalGet,
    ),
    spec(
        "POST",
        "/api/v1/debug/{service}/{method}",
        "/api/v1/debug/{service}/{method}",
        CoreOperation::DebugGlobalPost,
    ),
    spec(
        "GET",
        "/api/v1/debug/session/{session_id}/{service}/{method}",
        "/api/v1/debug/session/{session_id}/{service}/{method}",
        CoreOperation::DebugSessionGet,
    ),
    spec(
        "POST",
        "/api/v1/debug/session/{session_id}/{service}/{method}",
        "/api/v1/debug/session/{session_id}/{service}/{method}",
        CoreOperation::DebugSessionPost,
    ),
    spec(
        "GET",
        "/api/v1/debug/session/{session_id}/agent/{agent_id}/{service}/{method}",
        "/api/v1/debug/session/{session_id}/agent/{agent_id}/{service}/{method}",
        CoreOperation::DebugAgentGet,
    ),
    spec(
        "POST",
        "/api/v1/debug/session/{session_id}/agent/{agent_id}/{service}/{method}",
        "/api/v1/debug/session/{session_id}/agent/{agent_id}/{service}/{method}",
        CoreOperation::DebugAgentPost,
    ),
];

const fn spec(
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
    let mut specs = CORE_ROUTES.to_vec();
    if enable_terminals {
        specs.extend_from_slice(TERMINAL_ROUTES);
    }
    if debug_endpoints {
        specs.extend_from_slice(DEBUG_ROUTES);
    }
    specs
}

pub fn register_core_routes(
    mut router: Router<Arc<AppState>>,
    state: &AppState,
) -> Router<Arc<AppState>> {
    for route in core_route_specs(state.debug_endpoints, state.enable_terminals) {
        let filter = method_filter(route.method);
        router = router.route(
            route.runtime_path,
            on(filter, core_handler).layer(Extension(route.operation)),
        );
    }
    router
}

async fn core_handler(
    State(state): State<Arc<AppState>>,
    Extension(operation): Extension<CoreOperation>,
    Extension(request_id): Extension<RequestId>,
    request: Request,
) -> Response {
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

fn method_filter(method: &str) -> MethodFilter {
    match method {
        "GET" => MethodFilter::GET,
        "POST" => MethodFilter::POST,
        "PATCH" => MethodFilter::PATCH,
        "DELETE" => MethodFilter::DELETE,
        _ => panic!("unsupported route method: {method}"),
    }
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
        let mut expected = include_str!("api_surface.txt")
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
