use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::{get, post};

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/models",
        "/api/v1/models",
        CoreOperation::ListModels,
    ),
    route(
        "POST",
        "/api/v1/models/{tail}",
        "/api/v1/models/{tail}",
        CoreOperation::ModelAction,
    ),
    route(
        "GET",
        "/api/v1/providers",
        "/api/v1/providers",
        CoreOperation::ListProviders,
    ),
    route(
        "GET",
        "/api/v1/providers/{item}",
        "/api/v1/providers/{provider_id}",
        CoreOperation::GetProvider,
    ),
    route(
        "POST",
        "/api/v1/providers:refresh",
        "/api/v1/providers{action}",
        CoreOperation::ProviderCollectionAction,
    ),
    route(
        "POST",
        "/api/v1/providers:refresh_oauth",
        "/api/v1/providers{action}",
        CoreOperation::ProviderCollectionAction,
    ),
    route(
        "POST",
        "/api/v1/providers/{item}",
        "/api/v1/providers/{tail}",
        CoreOperation::ProviderAction,
    ),
];

async fn list_models(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ListModels, request).await
}

async fn model_action(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ModelAction, request).await
}

async fn list_providers(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ListProviders, request).await
}

async fn get_provider(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::GetProvider, request).await
}

async fn refresh_providers(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ProviderCollectionAction, request).await
}

async fn refresh_oauth_providers(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ProviderCollectionAction, request).await
}

async fn provider_action(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ProviderAction, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/api/v1/models", get(list_models))
        .route("/api/v1/models/{tail}", post(model_action))
        .route("/api/v1/providers", get(list_providers))
        .route("/api/v1/providers/{item}", get(get_provider))
        .route("/api/v1/providers:refresh", post(refresh_providers))
        .route(
            "/api/v1/providers:refresh_oauth",
            post(refresh_oauth_providers),
        )
        .route("/api/v1/providers/{item}", post(provider_action))
}
