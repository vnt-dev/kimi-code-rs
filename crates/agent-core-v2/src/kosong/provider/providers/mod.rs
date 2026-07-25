pub mod kimi;
pub mod standard;

use super::provider_definition::ProviderDefinitionRegistryError;

pub fn ensure_provider_definitions_registered() -> Result<(), ProviderDefinitionRegistryError> {
    standard::ensure_standard_provider_definitions_registered()?;
    kimi::ensure_kimi_provider_definitions_registered()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kosong::protocol::identity::Protocol;
    use crate::kosong::provider::provider_definition::{
        get_provider_definition, get_provider_definitions,
    };

    #[test]
    fn aggregate_registration_includes_both_kimi_transports_exactly_once() {
        ensure_provider_definitions_registered().unwrap();
        ensure_provider_definitions_registered().unwrap();
        assert_eq!(get_provider_definitions("kimi").unwrap().len(), 2);
        assert!(
            get_provider_definition("kimi", Some(Protocol::OpenAi))
                .unwrap()
                .is_some()
        );
        assert!(
            get_provider_definition("kimi", Some(Protocol::Anthropic))
                .unwrap()
                .is_some()
        );
    }
}
