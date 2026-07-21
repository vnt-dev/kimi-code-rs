use tokio::io::{self, AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader, Lines};

// Original:
//   apps/kimi-code/src/utils/process/stdin.ts
//   readStdinText()
pub async fn read_stdin_text() -> io::Result<String> {
    read_text(tokio::io::stdin()).await
}

pub async fn read_text<R>(mut reader: R) -> io::Result<String>
where
    R: AsyncRead + Unpin,
{
    let mut text = String::new();
    reader.read_to_string(&mut text).await?;
    Ok(text.trim().to_owned())
}

// Original:
//   apps/kimi-code/src/utils/process/stdin.ts
//   createStdinLineReader()
pub fn create_stdin_line_reader() -> Lines<BufReader<tokio::io::Stdin>> {
    create_line_reader(tokio::io::stdin())
}

pub fn create_line_reader<R>(reader: R) -> Lines<BufReader<R>>
where
    R: AsyncRead,
{
    BufReader::new(reader).lines()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_all_utf8_text_and_trims_outer_whitespace() {
        assert_eq!(
            read_text("  hello\nworld \r\n".as_bytes())
                .await
                .expect("stdin text"),
            "hello\nworld"
        );
        assert_eq!(
            read_text("\u{2003}trim me\u{2003}".as_bytes())
                .await
                .expect("unicode whitespace"),
            "trim me"
        );
    }

    #[tokio::test]
    async fn line_reader_handles_lf_crlf_empty_and_unterminated_final_lines() {
        let mut lines = create_line_reader("one\r\n\ntwo\nthree".as_bytes());
        assert_eq!(
            lines.next_line().await.expect("line"),
            Some("one".to_owned())
        );
        assert_eq!(lines.next_line().await.expect("line"), Some(String::new()));
        assert_eq!(
            lines.next_line().await.expect("line"),
            Some("two".to_owned())
        );
        assert_eq!(
            lines.next_line().await.expect("line"),
            Some("three".to_owned())
        );
        assert_eq!(lines.next_line().await.expect("eof"), None);
    }
}
