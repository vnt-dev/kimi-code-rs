use std::{
    io,
    path::{Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use futures_util::StreamExt;
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

use crate::{
    agent::context_memory::ContextFileAttachment,
    app::file::{FileServiceContract, GetResult},
    kosong::contract::message::ContentPart,
};

use super::{PromptFilePart, PromptInputPart, PromptMediaFilePart};

const ATTACHMENTS_DIR: &str = "attachments";
const ATTACHMENT_NAME_MAX: usize = 100;

pub struct ResolvedPromptInput {
    pub content: Vec<ContentPart>,
    pub attachments: Vec<ContextFileAttachment>,
}

pub async fn resolve_prompt_attachments(
    input: Vec<PromptInputPart>,
    files: &dyn FileServiceContract,
    session_dir: impl AsRef<Path>,
) -> Result<ResolvedPromptInput, Box<dyn std::error::Error + Send + Sync>> {
    let attachments_dir = session_dir.as_ref().join(ATTACHMENTS_DIR);
    let mut content = Vec::with_capacity(input.len());
    let mut attachments = Vec::new();

    for part in input {
        match part {
            PromptInputPart::Content(part) => content.push(part),
            PromptInputPart::File(PromptFilePart::File { file_id, .. }) => {
                // Uploaded metadata is authoritative. The name, media type,
                // and size supplied with the prompt are display hints only
                // and must not override the stored file record.
                let file = files.get(&file_id).await?;
                let path = materialize_attachment(&file, &attachments_dir).await?;
                let model_text = build_attached_file_notice(
                    &file.meta.name,
                    &file.meta.media_type,
                    file.meta.size,
                    &path,
                );
                content.push(ContentPart::Text {
                    text: model_text.clone(),
                });
                attachments.push(ContextFileAttachment {
                    file_id: file.meta.id,
                    name: file.meta.name,
                    media_type: file.meta.media_type,
                    size: file.meta.size,
                    model_text,
                });
            }
            PromptInputPart::MediaFile(media) => {
                let (file_id, kind) = match media {
                    PromptMediaFilePart::Image { file_id } => (file_id, MediaKind::Image),
                    PromptMediaFilePart::Audio { file_id } => (file_id, MediaKind::Audio),
                    PromptMediaFilePart::Video { file_id } => (file_id, MediaKind::Video),
                };
                let file = files.get(&file_id).await?;
                let data_url = uploaded_media_data_url(&file).await?;
                let media_url = crate::kosong::contract::message::MediaUrl {
                    url: data_url,
                    id: Some(file.meta.id),
                };
                content.push(match kind {
                    MediaKind::Image => ContentPart::ImageUrl {
                        image_url: media_url,
                    },
                    MediaKind::Audio => ContentPart::AudioUrl {
                        audio_url: media_url,
                    },
                    MediaKind::Video => ContentPart::VideoUrl {
                        video_url: media_url,
                    },
                });
            }
        }
    }

    Ok(ResolvedPromptInput {
        content,
        attachments,
    })
}

#[derive(Clone, Copy)]
enum MediaKind {
    Image,
    Audio,
    Video,
}

async fn uploaded_media_data_url(file: &GetResult) -> io::Result<String> {
    let capacity = usize::try_from(file.meta.size)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "uploaded media is too large"))?;
    let mut bytes = Vec::with_capacity(capacity);
    let mut stream = (file.stream)(None);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(io::Error::other)?;
        if bytes.len().saturating_add(chunk.len()) > capacity {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "uploaded file {} declared {} bytes but streamed more data",
                    file.meta.id, file.meta.size
                ),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() != capacity {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "uploaded file {} declared {} bytes but streamed {} bytes",
                file.meta.id,
                file.meta.size,
                bytes.len()
            ),
        ));
    }
    Ok(format!(
        "data:{};base64,{}",
        file.meta.media_type,
        BASE64_STANDARD.encode(bytes)
    ))
}

pub fn sanitize_attachment_name(name: &str) -> String {
    let cleaned = name
        .chars()
        .map(|character| match character {
            '/' | '\\' => '_',
            character => character,
        })
        .filter(|character| !character.is_control())
        .collect::<String>();
    let cleaned = cleaned
        .trim_start_matches('.')
        .trim()
        .chars()
        .take(ATTACHMENT_NAME_MAX)
        .collect::<String>();
    if cleaned.is_empty() {
        "attachment".into()
    } else {
        cleaned
    }
}

async fn materialize_attachment(file: &GetResult, dir: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(dir).await?;
    let target = dir.join(format!(
        "{}-{}",
        file.meta.id,
        sanitize_attachment_name(&file.meta.name)
    ));
    if fs::metadata(&target)
        .await
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() == file.meta.size)
    {
        return Ok(target);
    }

    let temporary = dir.join(format!(".{}.{}.tmp", file.meta.id, Uuid::new_v4()));
    let result = async {
        let mut output = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .await?;
        let mut stream = (file.stream)(None);
        let mut written = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(io::Error::other)?;
            written = written.saturating_add(chunk.len() as u64);
            output.write_all(&chunk).await?;
        }
        output.flush().await?;
        drop(output);

        if written != file.meta.size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "uploaded file {} declared {} bytes but streamed {written} bytes",
                    file.meta.id, file.meta.size
                ),
            ));
        }

        if fs::metadata(&target).await.is_ok() {
            fs::remove_file(&target).await?;
        }
        fs::rename(&temporary, &target).await?;
        Ok(target.clone())
    }
    .await;

    if result.is_err() {
        let _ = fs::remove_file(&temporary).await;
    }
    result
}

fn build_attached_file_notice(name: &str, media_type: &str, size: u64, path: &Path) -> String {
    format!(
        "Attached file \"{name}\" ({media_type}, {size} bytes): {} — open it with the Read tool",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use futures_util::stream;

    use crate::{
        agent::rpc::PromptFilePart,
        app::file::{FileByteStream, FileMeta, FileServiceError, FileServiceResult, SaveOptions},
    };

    use super::*;

    #[derive(Clone)]
    struct StubFileService {
        result: GetResult,
    }

    #[async_trait]
    impl FileServiceContract for StubFileService {
        async fn save(
            &self,
            _source: FileByteStream,
            _filename: &str,
            _options: Option<SaveOptions>,
        ) -> FileServiceResult<FileMeta> {
            unreachable!("save is not used by attachment resolution")
        }

        async fn get(&self, _file_id: &str) -> FileServiceResult<GetResult> {
            Ok(self.result.clone())
        }

        async fn delete(&self, _file_id: &str) -> FileServiceResult<()> {
            unreachable!("delete is not used by attachment resolution")
        }
    }

    fn stub_file(name: &str, bytes: &'static [u8]) -> StubFileService {
        let bytes = bytes.to_vec();
        StubFileService {
            result: GetResult {
                meta: FileMeta {
                    id: "f_stored".into(),
                    name: name.into(),
                    media_type: "application/xml".into(),
                    size: bytes.len() as u64,
                    created_at: "2026-01-01T00:00:00.000Z".into(),
                    expires_at: None,
                },
                stream: Arc::new(move |_| {
                    Box::pin(stream::once({
                        let bytes = bytes.clone();
                        async move { Ok::<_, FileServiceError>(bytes) }
                    }))
                }),
            },
        }
    }

    #[test]
    fn sanitizes_untrusted_attachment_names() {
        assert_eq!(
            sanitize_attachment_name("../../etc\\evil.xml"),
            "_.._etc_evil.xml"
        );
        assert_eq!(sanitize_attachment_name("...\0\r\n"), "attachment");
        assert_eq!(
            sanitize_attachment_name(&"a".repeat(120)).chars().count(),
            ATTACHMENT_NAME_MAX
        );
    }

    #[tokio::test]
    async fn materializes_file_and_keeps_authoritative_metadata() {
        let root = std::env::temp_dir().join(format!("kimi-attachment-{}", Uuid::new_v4()));
        let service = stub_file("../../stored.xml", b"<root />");
        let resolved = resolve_prompt_attachments(
            vec![PromptInputPart::File(PromptFilePart::File {
                file_id: "f_client".into(),
                name: "spoofed.xlsx".into(),
                media_type: "application/spoofed".into(),
                size: 999,
            })],
            &service,
            &root,
        )
        .await
        .unwrap();

        let expected_path = root.join(ATTACHMENTS_DIR).join("f_stored-_.._stored.xml");
        assert_eq!(fs::read(&expected_path).await.unwrap(), b"<root />");
        assert_eq!(resolved.attachments.len(), 1);
        assert_eq!(resolved.attachments[0].file_id, "f_stored");
        assert_eq!(resolved.attachments[0].name, "../../stored.xml");
        assert_eq!(resolved.attachments[0].media_type, "application/xml");
        assert_eq!(resolved.attachments[0].size, 8);
        assert!(matches!(
            &resolved.content[0],
            ContentPart::Text { text }
                if text.contains(expected_path.to_string_lossy().as_ref())
                    && text.contains("open it with the Read tool")
        ));

        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn resolves_uploaded_media_to_a_data_url() {
        let service = stub_file("clip.bin", b"media");
        let resolved = resolve_prompt_attachments(
            vec![PromptInputPart::MediaFile(PromptMediaFilePart::Audio {
                file_id: "f_client".into(),
            })],
            &service,
            std::env::temp_dir(),
        )
        .await
        .unwrap();

        assert!(resolved.attachments.is_empty());
        assert!(matches!(
            &resolved.content[0],
            ContentPart::AudioUrl { audio_url }
                if audio_url.url == "data:application/xml;base64,bWVkaWE="
                    && audio_url.id.as_deref() == Some("f_stored")
        ));
    }
}
