use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use kimi_code_agent_core_v2::app::config::{
    ConfigTarget, ResolvedConfig, camel_to_snake, snake_to_camel,
};
use kimi_code_agent_core_v2::app::event::GlobalDomainEvent;
use kimi_code_protocol::{ErrorCode, PatchConfigRequest, err_envelope, ok_envelope};
use serde_json::{Map, Value, json};

use super::{RouteSpec, route};
use crate::web::{AppState, CoreOperation, middleware::RequestId};

pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/config",
        "/api/v1/config",
        CoreOperation::GetConfig,
    ),
    route(
        "POST",
        "/api/v1/config",
        "/api/v1/config",
        CoreOperation::UpdateConfig,
    ),
];

// Original: packages/kap-server/src/routes/config.ts, GET /config.
async fn get_config(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let Some(config) = state.config_service.as_ref() else {
        return missing_service("ConfigService", request_id.0);
    };
    if let Err(error) = config.ready().await {
        return internal_error(error.to_string(), request_id.0);
    }
    Json(ok_envelope(
        to_config_response(config.get_all()),
        request_id.0,
    ))
    .into_response()
}

// Original: packages/kap-server/src/routes/config.ts, POST /config.
async fn update_config(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<PatchConfigRequest>, JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(error) => {
            return Json(err_envelope(
                ErrorCode::ValidationFailed,
                error.body_text(),
                request_id.0,
                None,
            ))
            .into_response();
        }
    };
    let (Some(config), Some(events)) =
        (state.config_service.as_ref(), state.event_service.as_ref())
    else {
        return missing_service("ConfigService/EventService", request_id.0);
    };
    if let Err(error) = config.ready().await {
        return validation_error(error.to_string(), request_id.0);
    }

    let Value::Object(patch) =
        serde_json::to_value(body).unwrap_or_else(|_| Value::Object(Map::new()))
    else {
        unreachable!("PatchConfigRequest always serializes as an object");
    };
    let changed_fields = patch.keys().cloned().collect::<Vec<_>>();
    let mut camel_patch = match convert_keys_snake_to_camel(Value::Object(patch.clone())) {
        Value::Object(patch) => patch,
        _ => unreachable!("object conversion preserves the outer object"),
    };
    if camel_patch.get("yolo") == Some(&Value::Bool(true)) {
        camel_patch.insert("defaultPermissionMode".into(), Value::String("yolo".into()));
    }
    camel_patch.shift_remove("yolo");

    for (domain, value) in camel_patch {
        if let Err(error) = config.set(&domain, Some(value), ConfigTarget::User).await {
            return validation_error(error.to_string(), request_id.0);
        }
    }

    let response = to_config_response(config.get_all());
    events.publish(GlobalDomainEvent {
        event_type: "event.config.changed".into(),
        payload: json!({
            "changedFields": changed_fields,
            "config": response,
        }),
    });
    Json(ok_envelope(response, request_id.0)).into_response()
}

fn to_config_response(resolved: ResolvedConfig) -> Value {
    let mut wire = Map::new();
    for (domain, value) in &resolved {
        wire.insert(
            camel_to_snake(domain),
            if domain == "providers" {
                to_provider_responses(value)
            } else {
                value.clone()
            },
        );
    }
    if let Some(Value::String(mode)) = resolved.get("defaultPermissionMode") {
        wire.insert("yolo".into(), Value::Bool(mode == "yolo"));
    }
    wire.entry("providers").or_insert_with(|| json!({}));
    Value::Object(wire)
}

fn to_provider_responses(value: &Value) -> Value {
    let Some(providers) = value.as_object() else {
        return json!({});
    };
    let providers = providers
        .iter()
        .map(|(id, raw)| {
            let provider = raw.as_object();
            let provider_type = provider
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut response = Map::from_iter([
                ("type".into(), Value::String(provider_type.into())),
                (
                    "has_api_key".into(),
                    Value::Bool(
                        provider
                            .and_then(|value| value.get("apiKey"))
                            .and_then(non_empty)
                            .is_some()
                            || provider.is_some_and(|value| value.contains_key("oauth")),
                    ),
                ),
            ]);
            for (source, target) in [("baseUrl", "base_url"), ("defaultModel", "default_model")] {
                if let Some(value) = provider
                    .and_then(|provider| provider.get(source))
                    .and_then(non_empty)
                {
                    response.insert(target.into(), Value::String(value.into()));
                }
            }
            (id.clone(), Value::Object(response))
        })
        .collect();
    Value::Object(providers)
}

fn non_empty(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn convert_keys_snake_to_camel(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(convert_keys_snake_to_camel)
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (snake_to_camel(&key), convert_keys_snake_to_camel(value)))
                .collect(),
        ),
        value => value,
    }
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

fn internal_error(message: String, request_id: String) -> Response {
    Json(err_envelope(
        ErrorCode::InternalError,
        message,
        request_id,
        None,
    ))
    .into_response()
}

fn missing_service(service: &str, request_id: String) -> Response {
    // MIGRATION-TODO:
    // Original start.ts resolves these handles from the assembled Scope. The
    // Rust application Scope is not composed yet, so start_server accepts
    // explicit handles until that composition root is migrated.
    internal_error(format!("{service} is not configured"), request_id)
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/api/v1/config", get(get_config))
        .route("/api/v1/config", post(update_config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_v2_config_to_v1_wire_shape_and_redacts_credentials() {
        let resolved = json!({
            "providers": {
                "kimi": {
                    "type": "openai",
                    "baseUrl": " https://api.example ",
                    "defaultModel": " kimi-k2 ",
                    "apiKey": " secret "
                },
                "oauth": {"type": "managed", "oauth": null},
                "invalid": "not-an-object"
            },
            "defaultPermissionMode": "yolo",
            "extraSkillDirs": ["/skills"]
        })
        .as_object()
        .unwrap()
        .clone();
        assert_eq!(
            to_config_response(resolved),
            json!({
                "providers": {
                    "kimi": {
                        "type": "openai",
                        "base_url": "https://api.example",
                        "default_model": "kimi-k2",
                        "has_api_key": true
                    },
                    "oauth": {"type": "managed", "has_api_key": true},
                    "invalid": {"type": "", "has_api_key": false}
                },
                "default_permission_mode": "yolo",
                "extra_skill_dirs": ["/skills"],
                "yolo": true
            })
        );
    }

    #[test]
    fn recursively_converts_patch_keys_like_typescript_helper() {
        assert_eq!(
            convert_keys_snake_to_camel(json!({
                "default_model": "k2",
                "providers": {"kimi": {"base_url": "https://api.example"}},
                "items": [{"nested_key": true}]
            })),
            json!({
                "defaultModel": "k2",
                "providers": {"kimi": {"baseUrl": "https://api.example"}},
                "items": [{"nestedKey": true}]
            })
        );
    }
}
