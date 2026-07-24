pub mod catalog;
pub mod completion_budget;
pub mod config_section;
pub mod contract;
pub mod discovery;
pub mod discovery_config_section;
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
    StaticAuthProvider, global_default_for_provider, model_ids_for_provider, to_protocol_model,
    to_protocol_model_fallback, to_protocol_provider,
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
