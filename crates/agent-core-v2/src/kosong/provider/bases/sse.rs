//! Shared incremental SSE frame decoder.
//!
//! Consolidates the frame-splitting logic that was previously duplicated in
//! the Anthropic, GoogleGenAI, OpenAI legacy and OpenAI responses transports
//! (~90 lines each). Frames are split on `\n\n` or `\r\n\r\n` while `data:`
//! payload lines are collected per event.

/// Incremental decoder that consumes raw transport bytes and yields complete
/// SSE event frames without re-scanning already-consumed bytes on every push.
pub struct SseFrameDecoder {
    buffer: Vec<u8>,
    /// Offset of the first unread byte; consumed bytes are compacted lazily
    /// on the next `push`.
    read: usize,
}

impl SseFrameDecoder {
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            read: 0,
        }
    }

    /// Append raw bytes from the transport. Already-consumed bytes are
    /// compacted away so the buffer stays bounded.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.read > 0 {
            self.buffer.drain(..self.read);
            self.read = 0;
        }
        self.buffer.extend_from_slice(bytes);
    }

    /// Consume and return the next complete event frame, if one is buffered.
    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        let (at, delimiter_len) = find_event_boundary(&self.buffer[self.read..])?;
        let frame = self.buffer[self.read..self.read + at].to_vec();
        self.read += at + delimiter_len;
        Some(frame)
    }

    /// Whether any unread bytes are still buffered.
    pub fn has_pending(&self) -> bool {
        self.read < self.buffer.len()
    }

    /// Return any remaining unread bytes as a final frame, for streams that
    /// end without a trailing blank line. Returns `None` when nothing is left.
    pub fn finish(&mut self) -> Option<Vec<u8>> {
        if self.read >= self.buffer.len() {
            return None;
        }
        let frame = self.buffer[self.read..].to_vec();
        self.buffer.clear();
        self.read = 0;
        Some(frame)
    }

    /// Drop all buffered state (used after a terminal `[DONE]` marker).
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.read = 0;
    }
}

impl Default for SseFrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Locate the first event boundary (`\n\n` or `\r\n\r\n`, whichever comes
/// first) in `buffer`, returning its position and delimiter length.
pub fn find_event_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(lf), Some(crlf)) if lf < crlf => Some((lf, 2)),
        (Some(_), Some(crlf)) => Some((crlf, 4)),
        (Some(lf), None) => Some((lf, 2)),
        (None, Some(crlf)) => Some((crlf, 4)),
        (None, None) => None,
    }
}

/// Extract the `data:` payload of a single event frame: all `data:` lines
/// concatenated with `\n`, each stripped of its leading whitespace.
pub fn extract_data(frame: &[u8]) -> Result<String, std::str::Utf8Error> {
    let text = std::str::from_utf8(frame)?;
    Ok(text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frames_on_lf_and_crlf_taking_the_first_boundary() {
        let mut decoder = SseFrameDecoder::new();
        decoder.push(b"data: one\r\n\r\ndata: two\n\n");
        assert_eq!(decoder.next_frame(), Some(b"data: one".to_vec()));
        assert_eq!(decoder.next_frame(), Some(b"data: two".to_vec()));
        assert_eq!(decoder.next_frame(), None);
    }

    #[test]
    fn collects_data_lines_and_ignores_non_data_lines() {
        let mut decoder = SseFrameDecoder::new();
        decoder.push(b"event: delta\r\ndata: {\"a\":1}\r\n: comment\r\n\r\ndata: [DONE]\n\n");
        let frame = decoder.next_frame().unwrap();
        assert_eq!(extract_data(&frame).unwrap(), "{\"a\":1}");
        let frame = decoder.next_frame().unwrap();
        assert_eq!(extract_data(&frame).unwrap(), "[DONE]");
        assert_eq!(decoder.next_frame(), None);
    }

    #[test]
    fn finish_returns_the_final_unterminated_frame() {
        let mut decoder = SseFrameDecoder::new();
        decoder.push(b"data: partial");
        assert_eq!(decoder.next_frame(), None);
        assert_eq!(decoder.finish(), Some(b"data: partial".to_vec()));
        assert_eq!(decoder.finish(), None);
    }
}
