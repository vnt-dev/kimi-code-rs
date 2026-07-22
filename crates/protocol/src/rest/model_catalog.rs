use serde::{Deserialize, Deserializer, Serialize};

use crate::validation::non_empty;
use crate::{ModelCatalogItem, ProviderCatalogItem, ProviderRefreshChange, ProviderRefreshFailure};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListModelsResponse {
    pub items: Vec<ModelCatalogItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListProvidersResponse {
    pub items: Vec<ProviderCatalogItem>,
}

pub type GetProviderResponse = ProviderCatalogItem;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetDefaultModelResponse {
    #[serde(deserialize_with = "non_empty")]
    pub default_model: String,
    pub model: ModelCatalogItem,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshProviderModelsResponse {
    pub changed: Vec<ProviderRefreshChange>,
    #[serde(deserialize_with = "deserialize_non_empty_strings")]
    pub unchanged: Vec<String>,
    pub failed: Vec<ProviderRefreshFailure>,
}

pub type RefreshOAuthProviderModelsResponse = RefreshProviderModelsResponse;

fn deserialize_non_empty_strings<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Vec::<String>::deserialize(deserializer)?;
    if values.iter().any(String::is_empty) {
        Err(serde::de::Error::custom("items must not be empty"))
    } else {
        Ok(values)
    }
}
