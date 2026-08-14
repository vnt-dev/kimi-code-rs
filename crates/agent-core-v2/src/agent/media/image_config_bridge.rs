//! Configuration-to-image-compression bridge.
//!
//! Original: `packages/agent-core-v2/src/agent/media/imageConfigBridge.ts`.

use std::{ops::Deref, sync::Arc};

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::{ServiceIdentifier, ServicesAccessorExt},
        lifecycle::{Disposable, DisposableStore, DisposeResult},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    app::config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
};

use super::{
    IMAGE_CONFIG_SCHEMA, IMAGE_SECTION, ImageConfig, set_configured_max_image_edge_px,
    set_configured_read_image_byte_budget,
};

pub struct ImageConfigBridge {
    disposables: DisposableStore,
}

impl ImageConfigBridge {
    // Original: ImageConfigBridge.constructor(). The weak capture prevents the
    // config event subscription from retaining the Agent-scoped bridge.
    pub fn new(config: ConfigServiceHandle) -> Arc<Self> {
        let service = Arc::new(Self {
            disposables: DisposableStore::new(),
        });
        service.push(config.get(IMAGE_SECTION));
        let weak = Arc::downgrade(&service);
        let subscription = config.on_did_section_change().subscribe(move |event| {
            if event.domain == IMAGE_SECTION
                && let Some(service) = weak.upgrade()
            {
                service.push(event.value.clone());
            }
        });
        service.disposables.add(subscription);
        service
    }

    // Original: ImageConfigBridge.push(). ConfigService has already validated
    // section data; parsing here still safely handles a malformed test or
    // extension event as an absent image config.
    fn push(&self, image: Option<serde_json::Value>) {
        let image = image
            .and_then(|value| IMAGE_CONFIG_SCHEMA.parse(&value).ok())
            .and_then(|value| serde_json::from_value::<ImageConfig>(value).ok());
        set_configured_max_image_edge_px(image.as_ref().and_then(|image| image.max_edge_px));
        set_configured_read_image_byte_budget(
            image.as_ref().and_then(|image| image.read_byte_budget),
        );
    }
}

impl Disposable for ImageConfigBridge {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

#[derive(Clone)]
pub struct ImageConfigBridgeHandle(pub Arc<ImageConfigBridge>);

impl Deref for ImageConfigBridgeHandle {
    type Target = ImageConfigBridge;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for ImageConfigBridgeHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const IMAGE_CONFIG_BRIDGE_ID: ServiceIdentifier<ImageConfigBridgeHandle> =
    ServiceIdentifier::new("imageConfigBridge");

// Original: registerScopedService(... ImageConfigBridge ..., Eager, "media").
pub fn register_image_config_bridge() {
    register_scoped_service(
        LifecycleScope::Agent,
        IMAGE_CONFIG_BRIDGE_ID,
        SyncDescriptor::new(|accessor| {
            let config = accessor.get(CONFIG_SERVICE_ID)?;
            Ok(ImageConfigBridgeHandle(ImageConfigBridge::new(
                (*config).clone(),
            )))
        })
        .disposable(),
        InstantiationType::Eager,
        "media",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identifier_matches_the_source_contract() {
        assert_eq!(IMAGE_CONFIG_BRIDGE_ID.to_string(), "imageConfigBridge");
    }
}
