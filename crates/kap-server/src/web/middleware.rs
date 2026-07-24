use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::Json;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_REQUEST_HEADERS, AUTHORIZATION, HOST, ORIGIN, VARY,
};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use kimi_code_protocol::err_envelope;

use crate::middleware::auth::{AuthDecision, authorize_request};
use crate::middleware::hostnames::{format_host_error_message, is_allowed_host};
use crate::middleware::origin::is_origin_allowed;
use crate::middleware::security_headers::security_headers;
use crate::request_id::{REQUEST_ID_HEADER, resolve_request_id};
use crate::security::bind_classify::BindClass;

use super::state::AppState;

pub const HOST_ERROR_CODE: i64 = 40_301;
const CORS_ALLOW_METHODS: &str = "GET, POST, PUT, PATCH, DELETE, OPTIONS";
const CORS_ALLOW_HEADERS: &str = "Content-Type, Authorization, X-Kimi-Client-Id, \
X-Kimi-Client-Name, X-Kimi-Client-Version, X-Kimi-Client-Ui-Mode";

#[derive(Debug, Clone)]
pub struct RequestId(pub String);

// Original:
//   middleware/hostnames.ts createHostCheck().onRequest()
//   middleware/origin.ts createOriginHook()
//   middleware/auth.ts createAuthHook()
//
// The checks intentionally remain in Host -> Origin -> auth order.
pub async fn boundary(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    let request_id = resolve_http_request_id(&request);
    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));

    let host = header_string(&request, HOST);
    if !is_allowed_host(host.as_deref(), &state.host_check) {
        return envelope_error(
            StatusCode::FORBIDDEN,
            HOST_ERROR_CODE,
            format_host_error_message(host.as_deref()),
            request_id,
        );
    }

    let origin = header_string(&request, ORIGIN);
    let origin_allowed =
        is_origin_allowed(origin.as_deref(), host.as_deref(), &state.allowed_origins);
    if request.method() == Method::OPTIONS {
        let mut response = StatusCode::NO_CONTENT.into_response();
        if origin.is_some() && origin_allowed {
            add_cors_headers(&mut response, origin.as_deref(), &request);
        }
        add_security_headers(&state, &mut response);
        return response;
    }

    if !state.disable_auth && request.uri().path() != "/api/v1/ws" {
        let remote_ip = request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|address| address.0.ip())
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
            .to_string();
        let authorization = header_string(&request, AUTHORIZATION);
        let decision = authorize_request(
            &state.auth_token_service,
            Some(&state.credential_validator),
            state.auth_failure_limiter.as_deref(),
            request.method().as_str(),
            request
                .uri()
                .path_and_query()
                .map_or(request.uri().path(), |value| value.as_str()),
            &remote_ip,
            authorization.as_deref(),
        )
        .await;
        match decision {
            Ok(AuthDecision::Bypassed | AuthDecision::Authorized { .. }) => {}
            Ok(AuthDecision::Rejected {
                status,
                code,
                message,
                ..
            }) => {
                return envelope_error(
                    StatusCode::from_u16(status).unwrap_or(StatusCode::UNAUTHORIZED),
                    code,
                    message,
                    request_id,
                );
            }
            Err(error) => {
                return envelope_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    50_000,
                    error.to_string(),
                    request_id,
                );
            }
        }
    }

    let cors_request_headers = header_string(&request, ACCESS_CONTROL_REQUEST_HEADERS);
    let mut response = next.run(request).await;
    if origin.is_some() && origin_allowed {
        add_cors_headers_values(
            &mut response,
            origin.as_deref(),
            cors_request_headers.as_deref(),
        );
    }
    add_security_headers(&state, &mut response);
    response
}

fn resolve_http_request_id(request: &Request) -> String {
    let mut headers = HashMap::new();
    let values = request
        .headers()
        .get_all(REQUEST_ID_HEADER)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if !values.is_empty() {
        headers.insert(REQUEST_ID_HEADER.to_owned(), values);
    }
    resolve_request_id(&headers)
}

fn header_string(request: &Request, name: HeaderName) -> Option<String> {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn envelope_error(
    status: StatusCode,
    code: i64,
    message: impl Into<String>,
    request_id: String,
) -> Response {
    (status, Json(err_envelope(code, message, request_id, None))).into_response()
}

fn add_cors_headers(response: &mut Response, origin: Option<&str>, request: &Request) {
    add_cors_headers_values(
        response,
        origin,
        header_string(request, ACCESS_CONTROL_REQUEST_HEADERS).as_deref(),
    );
}

fn add_cors_headers_values(
    response: &mut Response,
    origin: Option<&str>,
    requested_headers: Option<&str>,
) {
    let Some(origin) = origin.and_then(|value| HeaderValue::from_str(value).ok()) else {
        return;
    };
    response
        .headers_mut()
        .insert(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    response.headers_mut().insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static(CORS_ALLOW_METHODS),
    );
    if let Ok(value) = HeaderValue::from_str(requested_headers.unwrap_or(CORS_ALLOW_HEADERS)) {
        response
            .headers_mut()
            .insert(ACCESS_CONTROL_ALLOW_HEADERS, value);
    }
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static("Origin"));
}

fn add_security_headers(state: &AppState, response: &mut Response) {
    if state.exposure_class == BindClass::Loopback {
        return;
    }
    for (name, value) in security_headers(false) {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            response.headers_mut().insert(name, value);
        }
    }
}
