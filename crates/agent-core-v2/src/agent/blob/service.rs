use parking_lot::Mutex;
use std::sync::Arc;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        utils::hash::sha256_hex,
    },
    agent::scope_context::{AGENT_SCOPE_CONTEXT_ID, AgentScopeContext},
    kosong::contract::message::{ContentPart, MediaUrl},
    persistence::interface::{
        blob_store::{BLOB_STORE_SERVICE_ID, BlobStoreHandle, BlobStoreService},
        storage::StorageError,
    },
    wire::wire_service::WireBlobService,
};
use async_trait::async_trait;
use base64::{
    Engine,
    alphabet::STANDARD,
    engine::{GeneralPurpose, general_purpose::STANDARD as BASE64_STANDARD},
};

use super::{
    AGENT_BLOB_SERVICE_ID, AgentBlobServiceContract, AgentBlobServiceHandle, BLOBREF_PROTOCOL,
    ByteLruCache, MISSING_MEDIA_PLACEHOLDER,
};

const DEFAULT_THRESHOLD: usize = 4096;
const DEFAULT_MAX_CACHE_SIZE: usize = 50 * 1024 * 1024;

pub struct AgentBlobService {
    blobs: Arc<dyn BlobStoreService>,
    storage_scope: String,
    cache: Mutex<ByteLruCache>,
    threshold: usize,
}

impl AgentBlobService {
    // Original: agentBlobServiceImpl.ts, AgentBlobServiceImpl.constructor().
    pub fn new(blobs: BlobStoreHandle, agent: &AgentScopeContext) -> Self {
        Self {
            blobs: blobs.0,
            storage_scope: agent.scope(Some("blobs")),
            cache: Mutex::new(ByteLruCache::new(DEFAULT_MAX_CACHE_SIZE)),
            threshold: DEFAULT_THRESHOLD,
        }
    }

    async fn offload_content_part(
        &self,
        mut part: ContentPart,
    ) -> Result<ContentPart, StorageError> {
        let Some(media) = media_url_mut(&mut part) else {
            return Ok(part);
        };
        if let Some(url) = self.maybe_offload_string(&media.url).await? {
            media.url = url;
        }
        Ok(part)
    }

    async fn load_content_part(&self, mut part: ContentPart) -> ContentPart {
        let Some(media) = media_url_mut(&mut part) else {
            return part;
        };
        if !self.is_blob_ref(&media.url) {
            return part;
        }
        media.url = self
            .load_blob_ref_url(&media.url)
            .await
            .unwrap_or_else(|| MISSING_MEDIA_PLACEHOLDER.into());
        part
    }

    // Original: AgentBlobServiceImpl.loadBlobRefUrl().
    async fn load_blob_ref_url(&self, url: &str) -> Option<String> {
        let blob_ref = parse_blob_ref(url)?;
        let payload = self.read_blob(blob_ref.hash).await?;
        Some(format_data_uri(blob_ref.mime_type, &payload))
    }

    // Original: AgentBlobServiceImpl.readBlob(). Storage failures are
    // intentionally indistinguishable from missing blobs.
    async fn read_blob(&self, hash: &str) -> Option<Arc<[u8]>> {
        if let Some(cached) = self.cache.lock().get(hash) {
            return Some(cached);
        }
        let payload: Arc<[u8]> = self
            .blobs
            .get(&self.storage_scope, hash)
            .await
            .ok()??
            .into();
        self.cache.lock().set(hash.into(), Arc::clone(&payload));
        Some(payload)
    }

    // Original: AgentBlobServiceImpl.maybeOffloadString(). Threshold uses
    // JavaScript UTF-16 string length, not decoded byte length.
    async fn maybe_offload_string(&self, value: &str) -> Result<Option<String>, StorageError> {
        if self.is_blob_ref(value) {
            return Ok(None);
        }
        let Some((mime_type, payload)) = parse_data_uri(value) else {
            return Ok(None);
        };
        if payload.encode_utf16().count() < self.threshold {
            return Ok(None);
        }
        self.write_blob(mime_type, payload).await.map(Some)
    }

    // Original: AgentBlobServiceImpl.writeBlob(). Content addressing hashes
    // the base64 text as UTF-8, while the persisted value is its decoded bytes.
    async fn write_blob(
        &self,
        mime_type: &str,
        base64_payload: &str,
    ) -> Result<String, StorageError> {
        let hash = sha256_hex(base64_payload.as_bytes());
        let binary: Arc<[u8]> = decode_node_base64(base64_payload).into();
        self.blobs.put(&self.storage_scope, &hash, &binary).await?;
        self.cache.lock().set(hash.clone(), binary);
        Ok(format_blob_ref(mime_type, &hash))
    }

    async fn offload_wire_part(
        &self,
        mut part: serde_json::Value,
    ) -> Result<serde_json::Value, StorageError> {
        let Some(fields) = part.as_object_mut() else {
            return Ok(part);
        };
        for value in fields.values_mut() {
            let Some(url) = value
                .as_object()
                .and_then(|container| container.get("url"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
            else {
                continue;
            };
            let Some(updated) = self.maybe_offload_string(&url).await? else {
                continue;
            };
            if let Some(url) = value
                .as_object_mut()
                .and_then(|container| container.get_mut("url"))
            {
                *url = serde_json::Value::String(updated);
            }
        }
        Ok(part)
    }

    async fn load_wire_part(&self, mut part: serde_json::Value) -> serde_json::Value {
        let Some(fields) = part.as_object_mut() else {
            return part;
        };
        for value in fields.values_mut() {
            let Some(url) = value
                .as_object()
                .and_then(|container| container.get("url"))
                .and_then(serde_json::Value::as_str)
                .filter(|url| self.is_blob_ref(url))
                .map(str::to_owned)
            else {
                continue;
            };
            let updated = self
                .load_blob_ref_url(&url)
                .await
                .unwrap_or_else(|| MISSING_MEDIA_PLACEHOLDER.into());
            if let Some(url) = value
                .as_object_mut()
                .and_then(|container| container.get_mut("url"))
            {
                *url = serde_json::Value::String(updated);
            }
        }
        part
    }
}

#[async_trait]
impl AgentBlobServiceContract for AgentBlobService {
    // Original: AgentBlobServiceImpl.offloadParts(). Sequential awaits and
    // output order are preserved; consuming Vec keeps unchanged inputs zero-copy.
    async fn offload_parts(
        &self,
        parts: Vec<ContentPart>,
    ) -> Result<Vec<ContentPart>, StorageError> {
        let mut output = Vec::with_capacity(parts.len());
        for part in parts {
            output.push(self.offload_content_part(part).await?);
        }
        Ok(output)
    }

    // Original: AgentBlobServiceImpl.loadParts().
    async fn load_parts(&self, parts: Vec<ContentPart>) -> Vec<ContentPart> {
        let mut output = Vec::with_capacity(parts.len());
        for part in parts {
            output.push(self.load_content_part(part).await);
        }
        output
    }

    // Original: AgentBlobServiceImpl.isBlobRef().
    fn is_blob_ref(&self, url: &str) -> bool {
        url.starts_with(BLOBREF_PROTOCOL)
    }

    async fn offload_wire_parts(
        &self,
        parts: Vec<serde_json::Value>,
    ) -> Result<Vec<serde_json::Value>, String> {
        let mut output = Vec::with_capacity(parts.len());
        for value in parts {
            output.push(
                self.offload_wire_part(value)
                    .await
                    .map_err(|error| error.to_string())?,
            );
        }
        Ok(output)
    }

    async fn load_wire_parts(
        &self,
        parts: Vec<serde_json::Value>,
    ) -> Result<Vec<serde_json::Value>, String> {
        let mut output = Vec::with_capacity(parts.len());
        for value in parts {
            output.push(self.load_wire_part(value).await);
        }
        Ok(output)
    }
}

// Wire stores content parts as JSON values. Structural traversal rather than
// enum decoding preserves the source's behavior for legacy and extension fields.
#[async_trait]
impl WireBlobService for AgentBlobService {
    async fn offload_parts(
        &self,
        parts: Vec<serde_json::Value>,
    ) -> Result<Vec<serde_json::Value>, String> {
        AgentBlobServiceContract::offload_wire_parts(self, parts).await
    }

    async fn load_parts(
        &self,
        parts: Vec<serde_json::Value>,
    ) -> Result<Vec<serde_json::Value>, String> {
        AgentBlobServiceContract::load_wire_parts(self, parts).await
    }
}

fn media_url_mut(part: &mut ContentPart) -> Option<&mut MediaUrl> {
    match part {
        ContentPart::ImageUrl { image_url } => Some(image_url),
        ContentPart::AudioUrl { audio_url } => Some(audio_url),
        ContentPart::VideoUrl { video_url } => Some(video_url),
        ContentPart::Text { .. } | ContentPart::Think { .. } => None,
    }
}

// Original: agentBlobServiceImpl.ts, formatBlobRef().
fn format_blob_ref(mime_type: &str, hash: &str) -> String {
    format!("{BLOBREF_PROTOCOL}{mime_type};{hash}")
}

struct BlobRef<'a> {
    mime_type: &'a str,
    hash: &'a str,
}

// Original: agentBlobServiceImpl.ts, parseBlobRef(). Empty MIME types are
// valid; only the delimiter and nonempty hash are required.
fn parse_blob_ref(url: &str) -> Option<BlobRef<'_>> {
    let rest = url.strip_prefix(BLOBREF_PROTOCOL)?;
    let (mime_type, hash) = rest.split_once(';')?;
    (!hash.is_empty()).then_some(BlobRef { mime_type, hash })
}

// Original: agentBlobServiceImpl.ts, DATA_URI_HEADER_RE.
fn parse_data_uri(value: &str) -> Option<(&str, &str)> {
    let rest = value.strip_prefix("data:")?;
    let semicolon = rest.find(';')?;
    let mime_type = &rest[..semicolon];
    if mime_type.is_empty() || !rest[semicolon + 1..].starts_with("base64,") {
        return None;
    }
    Some((mime_type, &rest[semicolon + ";base64,".len()..]))
}

// Original: agentBlobServiceImpl.ts, formatDataUri().
fn format_data_uri(mime_type: &str, payload: &[u8]) -> String {
    format!(
        "data:{mime_type};base64,{}",
        BASE64_STANDARD.encode(payload)
    )
}

// Node Buffer.from(text, "base64") accepts URL-safe characters, ignores
// non-alphabet bytes, permits missing padding, and discards a one-character
// trailing quantum. Normalize those rules before using the Rust decoder.
fn decode_node_base64(value: &str) -> Vec<u8> {
    let mut normalized = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' => {
                normalized.push(char::from(byte));
            }
            b'-' => normalized.push('+'),
            b'_' => normalized.push('/'),
            b'=' => break,
            _ => {}
        }
    }
    if normalized.len() % 4 == 1 {
        normalized.pop();
    }
    let engine = GeneralPurpose::new(
        &STANDARD,
        base64::engine::general_purpose::GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::RequireNone),
    );
    engine.decode(normalized).unwrap_or_default()
}

// Original: agentBlobServiceImpl.ts, Agent-scope eager registration.
pub fn register_agent_blob_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_BLOB_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let blobs = accessor.get(BLOB_STORE_SERVICE_ID)?;
            let agent = accessor.get(AGENT_SCOPE_CONTEXT_ID)?;
            let service: Arc<dyn AgentBlobServiceContract> =
                Arc::new(AgentBlobService::new((*blobs).clone(), agent.as_ref()));
            Ok(AgentBlobServiceHandle(service))
        }),
        InstantiationType::Eager,
        "agentBlob",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        agent::scope_context::{AgentScopeContextInput, make_agent_scope_context},
        persistence::{
            backends::{
                memory::in_memory_storage_service::InMemoryStorageService,
                node_fs::blob_store_service::BlobStoreService as FileBlobStore,
            },
            interface::blob_store::BlobStoreService,
        },
    };

    fn media(url: impl Into<String>) -> MediaUrl {
        MediaUrl {
            url: url.into(),
            id: None,
        }
    }

    fn image(url: impl Into<String>) -> ContentPart {
        ContentPart::ImageUrl {
            image_url: media(url),
        }
    }

    fn video(url: impl Into<String>) -> ContentPart {
        ContentPart::VideoUrl {
            video_url: media(url),
        }
    }

    fn url(part: &ContentPart) -> &str {
        match part {
            ContentPart::ImageUrl { image_url } => &image_url.url,
            ContentPart::VideoUrl { video_url } => &video_url.url,
            _ => panic!("expected media part"),
        }
    }

    fn setup(scope: &str) -> (Arc<dyn BlobStoreService>, AgentBlobService) {
        let storage = Arc::new(InMemoryStorageService::default());
        let blobs: Arc<dyn BlobStoreService> = Arc::new(FileBlobStore::new(storage));
        let context = make_agent_scope_context(AgentScopeContextInput {
            agent_id: "agent".into(),
            agent_scope: scope.into(),
        });
        let service = AgentBlobService::new(BlobStoreHandle(blobs.clone()), &context);
        (blobs, service)
    }

    #[tokio::test]
    async fn offloads_every_large_media_part_and_round_trips_from_agent_scope() {
        let (blobs, service) = setup("sessions/s1/agents/a1");
        let payload = "A".repeat(5000);
        let uri = format!("data:image/png;base64,{payload}");
        let offloaded =
            AgentBlobServiceContract::offload_parts(&service, vec![image(&uri), video(&uri)])
                .await
                .unwrap();

        assert!(url(&offloaded[0]).starts_with("blobref:image/png;"));
        assert_eq!(url(&offloaded[0]), url(&offloaded[1]));
        assert_eq!(
            blobs
                .list("sessions/s1/agents/a1/blobs", None)
                .await
                .unwrap()
                .len(),
            1
        );
        let loaded = AgentBlobServiceContract::load_parts(&service, offloaded).await;
        assert_eq!(url(&loaded[0]), uri);
        assert_eq!(url(&loaded[1]), uri);
    }

    #[tokio::test]
    async fn a_fresh_agent_service_reads_persisted_bytes_and_keeps_scopes_isolated() {
        let storage = Arc::new(InMemoryStorageService::default());
        let blobs: Arc<dyn BlobStoreService> = Arc::new(FileBlobStore::new(storage));
        let context_a = make_agent_scope_context(AgentScopeContextInput {
            agent_id: "a".into(),
            agent_scope: "sessions/s1/agents/a".into(),
        });
        let context_b = make_agent_scope_context(AgentScopeContextInput {
            agent_id: "b".into(),
            agent_scope: "sessions/s1/agents/b".into(),
        });
        let writer = AgentBlobService::new(BlobStoreHandle(blobs.clone()), &context_a);
        let reader = AgentBlobService::new(BlobStoreHandle(blobs.clone()), &context_a);
        let isolated = AgentBlobService::new(BlobStoreHandle(blobs), &context_b);
        let payload = "A".repeat(5000);
        let uri = format!("data:image/jpeg;base64,{payload}");
        let reference = AgentBlobServiceContract::offload_parts(&writer, vec![image(&uri)])
            .await
            .unwrap();

        let loaded = AgentBlobServiceContract::load_parts(&reader, reference.clone()).await;
        assert_eq!(url(&loaded[0]), uri);
        let missing = AgentBlobServiceContract::load_parts(&isolated, reference).await;
        assert_eq!(url(&missing[0]), MISSING_MEDIA_PLACEHOLDER);
    }

    #[tokio::test]
    async fn leaves_small_and_non_media_values_unchanged() {
        let (_, service) = setup("");
        let parts = vec![
            image("data:image/png;base64,AQID"),
            ContentPart::Text {
                text: "text".into(),
            },
        ];
        assert_eq!(
            AgentBlobServiceContract::offload_parts(&service, parts.clone())
                .await
                .unwrap(),
            parts
        );
        assert!(!service.is_blob_ref("data:image/png;base64,AQID"));
        assert!(service.is_blob_ref("blobref:image/png;abc"));
    }

    #[tokio::test]
    async fn missing_or_malformed_blob_refs_render_placeholder() {
        let (_, service) = setup("");
        for reference in [
            "blobref:image/png;missing",
            "blobref:no-semicolon",
            "blobref:x;",
        ] {
            let loaded =
                AgentBlobServiceContract::load_parts(&service, vec![image(reference)]).await;
            assert_eq!(url(&loaded[0]), MISSING_MEDIA_PLACEHOLDER);
        }
    }

    #[test]
    fn helpers_preserve_wire_shapes_hashing_and_node_base64_tolerance() {
        assert_eq!(
            parse_data_uri("data:image/png;base64,AQID"),
            Some(("image/png", "AQID"))
        );
        assert_eq!(parse_data_uri("DATA:image/png;base64,AQID"), None);
        assert_eq!(parse_data_uri("data:;base64,AQID"), None);
        assert_eq!(parse_blob_ref("blobref:;hash").unwrap().mime_type, "");
        assert_eq!(decode_node_base64("YQ!junk"), [0x61, 0x08, 0xee, 0x9e]);
        assert_eq!(decode_node_base64("YQ==ignored"), b"a");
        assert_eq!(decode_node_base64("Y=Q"), b"");
        assert_eq!(
            sha256_hex(b"AQID"),
            "b70035bb783a47bf61ac3ff70b005308e167ee984365690e638c1481b8ca2936"
        );
    }

    #[tokio::test]
    async fn wire_adapter_rewrites_structural_media_containers_and_preserves_extensions() {
        let (_, service) = setup("");
        let payload = "A".repeat(5000);
        let input = serde_json::json!({
            "type": "future_media",
            "first": {"url": format!("data:image/png;base64,{payload}"), "detail": "high"},
            "second": {"url": "https://example.test/media"},
            "metadata": [1, 2, 3]
        });
        let offloaded = WireBlobService::offload_parts(&service, vec![input])
            .await
            .unwrap();
        assert!(
            offloaded[0]["first"]["url"]
                .as_str()
                .unwrap()
                .starts_with("blobref:image/png;")
        );
        assert_eq!(offloaded[0]["first"]["detail"], "high");
        assert_eq!(offloaded[0]["second"]["url"], "https://example.test/media");
        assert_eq!(offloaded[0]["metadata"], serde_json::json!([1, 2, 3]));

        let loaded = WireBlobService::load_parts(&service, offloaded)
            .await
            .unwrap();
        assert_eq!(
            loaded[0]["first"]["url"],
            format!("data:image/png;base64,{payload}")
        );
    }
}
