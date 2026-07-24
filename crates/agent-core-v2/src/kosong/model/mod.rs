pub mod catalog;
pub mod catalog_service;
pub mod completion_budget;
pub mod config_section;
pub mod contract;
pub mod discovery;
pub mod discovery_config_section;
pub mod discovery_service;
pub mod env_overlay;
pub mod errors;
pub mod host_request_headers;
pub mod inspection;
pub mod model_auth;
pub mod model_requester;
pub mod model_requester_impl;
pub mod model_service;
pub mod thinking;
pub mod types;

pub use catalog::{
    AuthProvider, AuthRequestOptions, Model, ModelCatalogItem, ModelPingResult,
    ProviderCatalogItem, ProviderCatalogStatus, ProviderCredentialState, SetDefaultModelResponse,
    StaticAuthProvider, build_protocol_provider_options, global_default_for_provider,
    has_configured_api_key, location_from_vertex_ai_base_url, model_ids_for_provider,
    profile_for_attribution, resolve_model_capabilities, resolve_outbound_headers,
    strip_trailing_v1, to_protocol_model, to_protocol_model_fallback, to_protocol_provider,
};
pub use catalog_service::{
    MODEL_CATALOG_SERVICE_ID, ModelCatalog, ModelCatalogContract, ModelCatalogError,
    ModelCatalogHandle, ModelCatalogResult, register_model_catalog,
};
pub use config_section::{
    MODELS_SCHEMA, models_from_toml, models_to_toml, register_models_config_section,
};
pub use discovery::{
    DiscoveryValidationError, PROVIDER_DISCOVERY_SERVICE_ID, ProviderDiscoveryResult,
    ProviderDiscoveryServiceContract, ProviderDiscoveryServiceHandle, ProviderRefreshChange,
    ProviderRefreshFailure, RefreshProviderModelsOptions, RefreshProviderModelsResponse,
    RefreshProviderModelsScope,
};
pub use discovery_config_section::{
    MODEL_CATALOG_CONFIG_SCHEMA, MODEL_CATALOG_SECTION, ModelCatalogConfig,
    register_model_catalog_config_section,
};
pub use discovery_service::{
    ProviderDiscoveryService, map_refresh_result, register_provider_discovery_service, without_keys,
};
pub use env_overlay::{
    ENV_MODEL_ALIAS_KEY, KIMI_MODEL_ENV_OVERLAY, KimiModelEnvOverlay,
    register_kimi_model_env_overlay,
};
pub use host_request_headers::{
    HOST_REQUEST_HEADERS_ID, HostRequestHeaders, host_request_headers_seed,
    register_host_request_headers,
};
pub use inspection::{
    InspectedAuth, InspectedAuthKind, InspectedModel, InspectedProvider,
    InspectedProviderDefinition, InspectedResolvedModel, InspectionAssemblyError, ModelInspection,
    ResolutionTraceCollector, TRACE, assemble_model_inspection, attribute_effective_fields,
    attribute_provider_options, mask_secret, redact_secrets,
};
pub use model_requester::{
    ModelRequestError, ModelRequestEvent, ModelRequestInput, ModelRequestParams,
    ModelRequestStream, ModelRequestTiming, ModelRequester, UploadVideoFuture,
    effective_max_completion_tokens,
};
pub use model_requester_impl::{ModelRequesterImpl, build_stream_timing};
pub use model_service::{ModelService, register_model_service};
