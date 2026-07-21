use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::utils::persistence::{PersistenceError, append_jsonl_line, read_jsonl_file};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputHistoryEntry {
    pub content: String,
}

// Original:
//   apps/kimi-code/src/utils/history/input-history.ts
//   loadInputHistory()
pub async fn load_input_history(file: &Path) -> Result<Vec<InputHistoryEntry>, PersistenceError> {
    read_jsonl_file(file, parse_entry).await
}

// Original:
//   apps/kimi-code/src/utils/history/input-history.ts
//   appendInputHistory()
pub async fn append_input_history(
    file: &Path,
    text: &str,
    last_content: Option<&str>,
) -> Result<bool, PersistenceError> {
    let content = text.trim();
    if content.is_empty() || last_content == Some(content) {
        return Ok(false);
    }
    append_jsonl_line(
        file,
        |value| {
            let entry = parse_entry(&value)
                .ok_or_else(|| "input history entry must contain string content".to_owned())?;
            serde_json::to_value(entry).map_err(|error| error.to_string())
        },
        &InputHistoryEntry {
            content: content.to_owned(),
        },
    )
    .await?;
    Ok(true)
}

fn parse_entry(value: &Value) -> Option<InputHistoryEntry> {
    Some(InputHistoryEntry {
        content: value.get("content")?.as_str()?.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp_file() -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!("kimi-input-history-{}-{id}", std::process::id()))
            .join("nested")
            .join("history.jsonl")
    }

    async fn cleanup(file: &Path) {
        if let Some(root) = file.parent().and_then(Path::parent) {
            let _ = tokio::fs::remove_dir_all(root).await;
        }
    }

    #[tokio::test]
    async fn missing_file_loads_as_empty_and_append_creates_parents() {
        let file = temp_file();
        assert_eq!(
            load_input_history(&file).await.expect("missing history"),
            Vec::<InputHistoryEntry>::new()
        );
        assert!(
            append_input_history(&file, "hello", None)
                .await
                .expect("append")
        );
        assert_eq!(
            load_input_history(&file).await.expect("history"),
            vec![InputHistoryEntry {
                content: "hello".to_owned()
            }]
        );
        cleanup(&file).await;
    }

    #[tokio::test]
    async fn corrupt_and_schema_invalid_lines_are_skipped() {
        let file = temp_file();
        tokio::fs::create_dir_all(file.parent().expect("parent"))
            .await
            .expect("parent");
        tokio::fs::write(
            &file,
            "{\"content\":\"first\"}\nnot-json\n{\"content\":7}\n{\"content\":\"!ls -la\",\"future\":true}\n",
        )
        .await
        .expect("history fixture");
        assert_eq!(
            load_input_history(&file).await.expect("history"),
            vec![
                InputHistoryEntry {
                    content: "first".to_owned()
                },
                InputHistoryEntry {
                    content: "!ls -la".to_owned()
                }
            ]
        );
        cleanup(&file).await;
    }

    #[tokio::test]
    async fn skips_empty_and_consecutive_duplicate_trimmed_content() {
        let file = temp_file();
        assert!(!append_input_history(&file, "", None).await.expect("empty"));
        assert!(
            !append_input_history(&file, "   ", None)
                .await
                .expect("blank")
        );
        assert!(append_input_history(&file, "a", None).await.expect("first"));
        assert!(
            !append_input_history(&file, " a ", Some("a"))
                .await
                .expect("duplicate")
        );
        assert!(
            append_input_history(&file, "b", Some("a"))
                .await
                .expect("second")
        );
        assert_eq!(
            load_input_history(&file).await.expect("history"),
            vec![
                InputHistoryEntry {
                    content: "a".to_owned()
                },
                InputHistoryEntry {
                    content: "b".to_owned()
                }
            ]
        );
        cleanup(&file).await;
    }
}
