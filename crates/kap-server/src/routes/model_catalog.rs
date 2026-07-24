use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/modelCatalog.ts, registerModelCatalogRoutes().
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
