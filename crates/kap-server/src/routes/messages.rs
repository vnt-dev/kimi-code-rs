use std::sync::Arc;

use axum::extract::rejection::QueryRejection;
use axum::extract::{Extension, Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use kimi_code_agent_core_v2::_base::errors::errors::Error2;
use kimi_code_agent_core_v2::agent::context_memory::protocol_message::MessageRole as CoreMessageRole;
use kimi_code_agent_core_v2::app::message_legacy::MessageListQuery;
use kimi_code_protocol::{ErrorCode, MessageRole, err_envelope, ok_envelope};
use serde::Deserialize;

use super::{RouteSpec, route};
use crate::web::{AppState, CoreOperation, middleware::RequestId};

pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/sessions/{session_id}/messages",
        "/api/v1/sessions/{session_id}/messages",
        CoreOperation::ListMessages,
    ),
    route(
        "GET",
        "/api/v1/sessions/{session_id}/messages/{message_id}",
        "/api/v1/sessions/{session_id}/messages/{message_id}",
        CoreOperation::GetMessage,
    ),
];

#[derive(Deserialize)]
struct RawListMessagesQuery {
    before_id: Option<String>,
    after_id: Option<String>,
    page_size: Option<String>,
    role: Option<MessageRole>,
}

// Original: packages/kap-server/src/routes/messages.ts, list handler.
async fn list_messages(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
    Path(session_id): Path<String>,
    query: Result<Query<RawListMessagesQuery>, QueryRejection>,
) -> Response {
    let Query(query) = match query {
        Ok(query) => query,
        Err(error) => return validation_error(error.body_text(), request_id.0),
    };
    if query.before_id.as_ref().is_some_and(String::is_empty)
        || query.after_id.as_ref().is_some_and(String::is_empty)
        || (query.before_id.is_some() && query.after_id.is_some())
    {
        return validation_error("invalid message cursor".into(), request_id.0);
    }
    let page_size = match query.page_size {
        Some(value) => match value.parse::<usize>() {
            Ok(value) if (1..=100).contains(&value) => Some(value),
            _ => {
                return validation_error(
                    "page_size must be between 1 and 100".into(),
                    request_id.0,
                );
            }
        },
        None => None,
    };
    let Some(service) = state.message_legacy_service.as_ref() else {
        return missing_service(request_id.0);
    };
    let query = MessageListQuery {
        before_id: query.before_id,
        after_id: query.after_id,
        page_size,
        role: query.role.map(to_core_role),
    };
    match service.list(&session_id, query).await {
        Ok(page) => Json(ok_envelope(page_to_value(page), request_id.0)).into_response(),
        Err(error) => mapped_error(error.as_ref(), request_id.0),
    }
}

// Original: packages/kap-server/src/routes/messages.ts, get handler.
async fn get_message(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
    Path((session_id, message_id)): Path<(String, String)>,
) -> Response {
    let Some(service) = state.message_legacy_service.as_ref() else {
        return missing_service(request_id.0);
    };
    match service.get(&session_id, &message_id).await {
        Ok(message) => Json(ok_envelope(message, request_id.0)).into_response(),
        Err(error) => mapped_error(error.as_ref(), request_id.0),
    }
}

fn to_core_role(role: MessageRole) -> CoreMessageRole {
    match role {
        MessageRole::User => CoreMessageRole::User,
        MessageRole::Assistant => CoreMessageRole::Assistant,
        MessageRole::Tool => CoreMessageRole::Tool,
        MessageRole::System => CoreMessageRole::System,
    }
}

fn page_to_value(
    page: kimi_code_agent_core_v2::app::message_legacy::PageResponse<
        kimi_code_agent_core_v2::agent::context_memory::protocol_message::ProtocolMessage,
    >,
) -> serde_json::Value {
    serde_json::json!({"items": page.items, "has_more": page.has_more})
}

fn validation_error(message: String, request_id: String) -> Response {
    Json(err_envelope(
        ErrorCode::ValidationFailed,
        message,
        request_id,
        None,
    ))
    .into_response()
}

fn mapped_error(error: &(dyn std::error::Error + 'static), request_id: String) -> Response {
    let (code, message) = match error.downcast_ref::<Error2>() {
        Some(error) if error.code == "session.not_found" => {
            (ErrorCode::SessionNotFound, error.to_string())
        }
        Some(error) if error.code == "message.not_found" => {
            (ErrorCode::MessageNotFound, error.to_string())
        }
        _ => (ErrorCode::InternalError, error.to_string()),
    };
    Json(err_envelope(code, message, request_id, None)).into_response()
}

fn missing_service(request_id: String) -> Response {
    // MIGRATION-TODO: replace optional injection when the app Scope is built.
    Json(err_envelope(
        ErrorCode::InternalError,
        "MessageLegacyService is not configured",
        request_id,
        None,
    ))
    .into_response()
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/api/v1/sessions/{session_id}/messages", get(list_messages))
        .route(
            "/api/v1/sessions/{session_id}/messages/{message_id}",
            get(get_message),
        )
}
