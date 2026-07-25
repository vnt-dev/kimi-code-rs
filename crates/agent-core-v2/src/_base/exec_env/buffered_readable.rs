//! Backpressure-preserving buffered async reader.
//!
//! Original: `packages/agent-core-v2/src/_base/execEnv/bufferedReadable.ts`,
//! `BufferedReadable`.

use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, DuplexStream, ReadBuf, duplex};
use tokio_util::sync::CancellationToken;

/// Matches the source wrapper's `Readable` high-water mark.
pub const BUFFERED_READABLE_CAPACITY: usize = 128 * 1024;

/// An [`AsyncRead`] wrapper that continuously drains a source up to a bounded
/// buffer.  Once the buffer is full, the forwarding task waits for the caller
/// to read more data; this preserves source backpressure instead of retaining
/// unbounded child-process output in memory.
///
/// The source implementation is event based.  Rust represents the same
/// producer/consumer boundary with `tokio::io::duplex`: a background task
/// reads the source and the returned stream remains readable after that source
/// reaches EOF.  Dropping this value cancels the task and drops its source.
pub struct BufferedReadable {
    reader: DuplexStream,
    cancellation: CancellationToken,
    source_error: Arc<Mutex<Option<io::Error>>>,
}

impl BufferedReadable {
    pub fn new<R>(source: R) -> Self
    where
        R: AsyncRead + Send + Unpin + 'static,
    {
        let (reader, writer) = duplex(BUFFERED_READABLE_CAPACITY);
        let cancellation = CancellationToken::new();
        let source_error = Arc::new(Mutex::new(None));
        tokio::spawn(forward(
            source,
            writer,
            cancellation.clone(),
            Arc::clone(&source_error),
        ));
        Self {
            reader,
            cancellation,
            source_error,
        }
    }
}

impl AsyncRead for BufferedReadable {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let remaining = buffer.remaining();
        match Pin::new(&mut self.reader).poll_read(context, buffer) {
            Poll::Ready(Ok(())) if buffer.remaining() == remaining => {
                let error = self
                    .source_error
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                match error {
                    Some(error) => Poll::Ready(Err(error)),
                    None => Poll::Ready(Ok(())),
                }
            }
            result => result,
        }
    }
}

impl Drop for BufferedReadable {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

async fn forward<R>(
    mut source: R,
    mut writer: DuplexStream,
    cancellation: CancellationToken,
    source_error: Arc<Mutex<Option<io::Error>>>,
) where
    R: AsyncRead + Send + Unpin + 'static,
{
    let result = async {
        let mut chunk = [0_u8; 8 * 1024];
        loop {
            let read = tokio::select! {
                _ = cancellation.cancelled() => return Ok(()),
                read = source.read(&mut chunk) => read,
            }?;
            if read == 0 {
                writer.shutdown().await?;
                return Ok(());
            }
            tokio::select! {
                _ = cancellation.cancelled() => return Ok(()),
                written = writer.write_all(&chunk[..read]) => written?,
            }
        }
    }
    .await;

    if let Err(error) = result
        && error.kind() != io::ErrorKind::BrokenPipe
        && !cancellation.is_cancelled()
    {
        *source_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

    use super::*;

    #[tokio::test]
    async fn retains_small_source_output_after_the_source_ends() {
        let (mut writer, source) = duplex(64);
        let mut buffered = BufferedReadable::new(source);
        writer.write_all(b"finished output").await.unwrap();
        writer.shutdown().await.unwrap();
        tokio::task::yield_now().await;

        let mut output = String::new();
        buffered.read_to_string(&mut output).await.unwrap();
        assert_eq!(output, "finished output");
    }

    #[tokio::test]
    async fn output_larger_than_one_read_is_forwarded_in_order() {
        let payload = vec![b'x'; BUFFERED_READABLE_CAPACITY + 17];
        let source = std::io::Cursor::new(payload.clone());
        let mut buffered = BufferedReadable::new(source);

        let mut output = Vec::new();
        buffered.read_to_end(&mut output).await.unwrap();
        assert_eq!(output, payload);
    }

    struct ErrorReader;

    impl AsyncRead for ErrorReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::other("source failed")))
        }
    }

    #[tokio::test]
    async fn forwards_source_errors_to_the_consumer() {
        let mut buffered = BufferedReadable::new(ErrorReader);
        let mut bytes = [0; 1];
        let error = buffered.read(&mut bytes).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(error.to_string(), "source failed");
    }
}
