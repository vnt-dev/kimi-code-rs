use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
};

use thiserror::Error;

use crate::crc32::crc32;

pub const MAGIC: [u8; 2] = [0x4d, 0x44];
pub const TYPE_SET: u8 = 1;
pub const TYPE_DEL: u8 = 2;
pub const TYPE_BATCH: u8 = 3;
pub const HEADER_SIZE: usize = 22;
pub const CRC_SIZE: usize = 4;
pub const MAX_KEY_LEN: usize = u16::MAX as usize;
const SUB_HEADER_SIZE: usize = 1 + 2 + 4 + 4 + 8;
const CRC_CHUNK_SIZE: usize = 1 << 20;
const MAGIC_SCAN_CHUNK_SIZE: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub frame_type: u8,
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    pub meta: Option<Vec<u8>>,
    pub expire_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchOp {
    pub op_type: u8,
    pub key: Vec<u8>,
    pub value: Option<Vec<u8>>,
    pub meta: Option<Vec<u8>>,
    pub expire_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CorruptionMode {
    #[default]
    Resync,
    Strict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseResult {
    pub frames: Vec<Frame>,
    pub corrupt_ranges: Vec<(u64, u64)>,
    pub eof_offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameRef {
    pub frame_type: u8,
    pub key: Vec<u8>,
    pub meta: Option<Vec<u8>>,
    pub expire_at: i64,
    pub frame_offset: u64,
    pub value_offset: u64,
    pub value_len: u32,
    pub frame_len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanFrameRefsResult {
    pub frames: Vec<FrameRef>,
    pub corrupt_ranges: Vec<(u64, u64)>,
    pub eof_offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchOpRef {
    pub op_type: u8,
    pub key: Vec<u8>,
    pub meta: Option<Vec<u8>>,
    pub expire_at: i64,
    pub value_offset: u64,
    pub value_len: u32,
}

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("key too large")]
    KeyTooLarge,
    #[error("too many batch operations")]
    TooManyBatchOperations,
    #[error("batch op header truncated")]
    BatchHeaderTruncated,
    #[error("batch op payload truncated")]
    BatchPayloadTruncated,
    #[error("{message}")]
    CorruptFrame { message: String, offset: u64 },
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl CodecError {
    pub fn corrupt_offset(&self) -> Option<u64> {
        match self {
            Self::CorruptFrame { offset, .. } => Some(*offset),
            _ => None,
        }
    }
}

// Original: packages/minidb/src/codec.ts, encodeFrame().
pub fn encode_frame(frame: &Frame) -> Result<Vec<u8>, CodecError> {
    if frame.key.len() > MAX_KEY_LEN {
        return Err(CodecError::KeyTooLarge);
    }
    let meta = frame.meta.as_deref().unwrap_or_default();
    let mut output =
        vec![0; HEADER_SIZE + frame.key.len() + frame.value.len() + meta.len() + CRC_SIZE];
    output[..2].copy_from_slice(&MAGIC);
    output[2] = frame.frame_type;
    output[3] = 0;
    output[4..6].copy_from_slice(&(frame.key.len() as u16).to_le_bytes());
    output[6..10].copy_from_slice(&(frame.value.len() as u32).to_le_bytes());
    output[10..14].copy_from_slice(&(meta.len() as u32).to_le_bytes());
    output[14..22].copy_from_slice(&frame.expire_at.to_le_bytes());
    let mut offset = HEADER_SIZE;
    output[offset..offset + frame.key.len()].copy_from_slice(&frame.key);
    offset += frame.key.len();
    output[offset..offset + frame.value.len()].copy_from_slice(&frame.value);
    offset += frame.value.len();
    output[offset..offset + meta.len()].copy_from_slice(meta);
    offset += meta.len();
    let checksum = crc32(&output[2..offset], 0);
    output[offset..offset + CRC_SIZE].copy_from_slice(&checksum.to_le_bytes());
    Ok(output)
}

// Original: packages/minidb/src/codec.ts, encodeBatchOps().
pub fn encode_batch_ops(operations: &[BatchOp]) -> Result<Vec<u8>, CodecError> {
    let count = u16::try_from(operations.len()).map_err(|_| CodecError::TooManyBatchOperations)?;
    let total = operations.iter().try_fold(2_usize, |total, operation| {
        if operation.key.len() > MAX_KEY_LEN {
            return Err(CodecError::KeyTooLarge);
        }
        Ok(total
            + SUB_HEADER_SIZE
            + operation.key.len()
            + operation.value.as_ref().map_or(0, Vec::len)
            + operation.meta.as_ref().map_or(0, Vec::len))
    })?;
    let mut output = Vec::with_capacity(total);
    output.extend_from_slice(&count.to_le_bytes());
    for operation in operations {
        let value = operation.value.as_deref().unwrap_or_default();
        let meta = operation.meta.as_deref().unwrap_or_default();
        output.push(operation.op_type);
        output.extend_from_slice(&(operation.key.len() as u16).to_le_bytes());
        output.extend_from_slice(&(value.len() as u32).to_le_bytes());
        output.extend_from_slice(&(meta.len() as u32).to_le_bytes());
        output.extend_from_slice(&operation.expire_at.to_le_bytes());
        output.extend_from_slice(&operation.key);
        output.extend_from_slice(value);
        output.extend_from_slice(meta);
    }
    Ok(output)
}

// Original: packages/minidb/src/codec.ts, decodeBatchOps().
pub fn decode_batch_ops(body: &[u8]) -> Result<Vec<BatchOp>, CodecError> {
    if body.len() < 2 {
        return Ok(Vec::new());
    }
    let count = u16::from_le_bytes(body[..2].try_into().expect("two-byte count"));
    let mut offset = 2;
    let mut operations = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let header = body
            .get(offset..offset + SUB_HEADER_SIZE)
            .ok_or(CodecError::BatchHeaderTruncated)?;
        let op_type = header[0];
        let key_len = u16::from_le_bytes(header[1..3].try_into().expect("key length")) as usize;
        let value_len = u32::from_le_bytes(header[3..7].try_into().expect("value length")) as usize;
        let meta_len = u32::from_le_bytes(header[7..11].try_into().expect("meta length")) as usize;
        let expire_at = i64::from_le_bytes(header[11..19].try_into().expect("expiration"));
        offset += SUB_HEADER_SIZE;
        let end = offset
            .checked_add(key_len)
            .and_then(|end| end.checked_add(value_len))
            .and_then(|end| end.checked_add(meta_len))
            .filter(|end| *end <= body.len())
            .ok_or(CodecError::BatchPayloadTruncated)?;
        let key = body[offset..offset + key_len].to_vec();
        offset += key_len;
        let value = body[offset..offset + value_len].to_vec();
        offset += value_len;
        let meta = (meta_len > 0).then(|| body[offset..offset + meta_len].to_vec());
        offset = end;
        operations.push(BatchOp {
            op_type,
            key,
            value: Some(value),
            meta,
            expire_at,
        });
    }
    Ok(operations)
}

fn read_frame_at(bytes: &[u8], position: usize) -> Option<(Frame, usize)> {
    let header = bytes.get(position..position + HEADER_SIZE)?;
    if header[..2] != MAGIC {
        return None;
    }
    let key_len = u16::from_le_bytes(header[4..6].try_into().ok()?) as usize;
    let value_len = u32::from_le_bytes(header[6..10].try_into().ok()?) as usize;
    let meta_len = u32::from_le_bytes(header[10..14].try_into().ok()?) as usize;
    let frame_len = HEADER_SIZE
        .checked_add(key_len)?
        .checked_add(value_len)?
        .checked_add(meta_len)?
        .checked_add(CRC_SIZE)?;
    let frame_bytes = bytes.get(position..position + frame_len)?;
    let stored = u32::from_le_bytes(frame_bytes[frame_len - CRC_SIZE..].try_into().ok()?);
    if stored != crc32(&frame_bytes[2..frame_len - CRC_SIZE], 0) {
        return None;
    }
    let payload = &frame_bytes[HEADER_SIZE..frame_len - CRC_SIZE];
    let value_start = key_len;
    let meta_start = value_start + value_len;
    Some((
        Frame {
            frame_type: header[2],
            key: payload[..key_len].to_vec(),
            value: payload[value_start..meta_start].to_vec(),
            meta: (meta_len > 0).then(|| payload[meta_start..].to_vec()),
            expire_at: i64::from_le_bytes(header[14..22].try_into().ok()?),
        },
        frame_len,
    ))
}

pub struct FrameParser {
    pending: Vec<u8>,
    offset: u64,
}

impl Default for FrameParser {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameParser {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            offset: 0,
        }
    }

    // Original: packages/minidb/src/codec.ts, FrameParser.feed().
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<Frame>, CodecError> {
        self.pending.extend_from_slice(chunk);
        let mut position = 0;
        let mut frames = Vec::new();
        loop {
            if self.pending.len() - position < HEADER_SIZE {
                break;
            }
            if self.pending[position..position + 2] != MAGIC {
                let next = self.pending[position + 1..]
                    .windows(2)
                    .position(|window| window == MAGIC)
                    .map(|relative| position + 1 + relative)
                    .ok_or_else(|| CodecError::CorruptFrame {
                        message: "magic not found".into(),
                        offset: self.offset + position as u64,
                    })?;
                position = next;
                continue;
            }
            let key_len = u16::from_le_bytes(
                self.pending[position + 4..position + 6]
                    .try_into()
                    .expect("key length"),
            ) as usize;
            let value_len = u32::from_le_bytes(
                self.pending[position + 6..position + 10]
                    .try_into()
                    .expect("value length"),
            ) as usize;
            let meta_len = u32::from_le_bytes(
                self.pending[position + 10..position + 14]
                    .try_into()
                    .expect("meta length"),
            ) as usize;
            let Some(frame_len) = HEADER_SIZE
                .checked_add(key_len)
                .and_then(|length| length.checked_add(value_len))
                .and_then(|length| length.checked_add(meta_len))
                .and_then(|length| length.checked_add(CRC_SIZE))
            else {
                return Err(CodecError::CorruptFrame {
                    message: "frame length overflow".into(),
                    offset: self.offset + position as u64,
                });
            };
            if self.pending.len() - position < frame_len {
                break;
            }
            let (frame, _) =
                read_frame_at(&self.pending, position).ok_or_else(|| CodecError::CorruptFrame {
                    message: format!("crc mismatch at offset {}", self.offset + position as u64),
                    offset: self.offset + position as u64,
                })?;
            frames.push(frame);
            position += frame_len;
            self.offset += frame_len as u64;
        }
        if position > 0 {
            self.pending.drain(..position);
        }
        Ok(frames)
    }

    // Original: packages/minidb/src/codec.ts, FrameParser.finish().
    pub fn finish(&mut self) -> Result<u64, CodecError> {
        if self.pending.is_empty() {
            return Ok(self.offset);
        }
        let trailing = self.pending.len();
        self.pending.clear();
        Err(CodecError::CorruptFrame {
            message: format!("torn tail: {trailing} trailing byte(s)"),
            offset: self.offset,
        })
    }
}

// Original: packages/minidb/src/codec.ts, parseBuffer().
pub fn parse_buffer(bytes: &[u8], mode: CorruptionMode) -> ParseResult {
    let mut frames = Vec::new();
    let mut corrupt_ranges = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        if let Some((frame, frame_len)) = read_frame_at(bytes, position) {
            frames.push(frame);
            position += frame_len;
            continue;
        }
        if mode == CorruptionMode::Strict {
            corrupt_ranges.push((position as u64, bytes.len() as u64));
            break;
        }
        let bad_start = position;
        let mut resume = None;
        let mut scan = position + 1;
        while scan + 1 < bytes.len() {
            let Some(relative) = bytes[scan..].windows(2).position(|window| window == MAGIC) else {
                break;
            };
            scan += relative;
            if read_frame_at(bytes, scan).is_some() {
                resume = Some(scan);
                break;
            }
            scan += 1;
        }
        let end = resume.unwrap_or(bytes.len());
        corrupt_ranges.push((bad_start as u64, end as u64));
        let Some(resume) = resume else {
            break;
        };
        position = resume;
    }
    ParseResult {
        frames,
        corrupt_ranges,
        eof_offset: position as u64,
    }
}

fn read_exact_at(file: &mut File, position: u64, bytes: &mut [u8]) -> io::Result<()> {
    file.seek(SeekFrom::Start(position))?;
    file.read_exact(bytes)
}

fn read_frame_ref_at(file: &mut File, position: u64, size: u64) -> io::Result<Option<FrameRef>> {
    if size.saturating_sub(position) < HEADER_SIZE as u64 {
        return Ok(None);
    }
    let mut header = [0_u8; HEADER_SIZE];
    read_exact_at(file, position, &mut header)?;
    if header[..2] != MAGIC {
        return Ok(None);
    }
    let key_len = u16::from_le_bytes(header[4..6].try_into().expect("key length")) as usize;
    let value_len = u32::from_le_bytes(header[6..10].try_into().expect("value length"));
    let meta_len = u32::from_le_bytes(header[10..14].try_into().expect("meta length")) as usize;
    let Some(frame_len) = (HEADER_SIZE as u64)
        .checked_add(key_len as u64)
        .and_then(|length| length.checked_add(value_len as u64))
        .and_then(|length| length.checked_add(meta_len as u64))
        .and_then(|length| length.checked_add(CRC_SIZE as u64))
    else {
        return Ok(None);
    };
    if size.saturating_sub(position) < frame_len {
        return Ok(None);
    }
    let mut checksum = 0;
    let mut checksum_position = position + 2;
    let mut remaining = frame_len - CRC_SIZE as u64 - 2;
    while remaining > 0 {
        let length = remaining.min(CRC_CHUNK_SIZE as u64) as usize;
        let mut chunk = vec![0; length];
        read_exact_at(file, checksum_position, &mut chunk)?;
        checksum = crc32(&chunk, checksum);
        checksum_position += length as u64;
        remaining -= length as u64;
    }
    let mut stored = [0_u8; CRC_SIZE];
    read_exact_at(file, position + frame_len - CRC_SIZE as u64, &mut stored)?;
    if u32::from_le_bytes(stored) != checksum {
        return Ok(None);
    }
    let key_start = position + HEADER_SIZE as u64;
    let value_offset = key_start + key_len as u64;
    let meta_start = value_offset + value_len as u64;
    let mut key = vec![0; key_len];
    read_exact_at(file, key_start, &mut key)?;
    let meta = if meta_len > 0 {
        let mut meta = vec![0; meta_len];
        read_exact_at(file, meta_start, &mut meta)?;
        Some(meta)
    } else {
        None
    };
    Ok(Some(FrameRef {
        frame_type: header[2],
        key,
        meta,
        expire_at: i64::from_le_bytes(header[14..22].try_into().expect("expiration")),
        frame_offset: position,
        value_offset,
        value_len,
        frame_len,
    }))
}

fn find_magic(file: &mut File, start: u64, size: u64) -> io::Result<Option<u64>> {
    let mut buffer = vec![0; MAGIC_SCAN_CHUNK_SIZE];
    let mut position = start;
    while position < size {
        let length = (size - position).min(buffer.len() as u64) as usize;
        file.seek(SeekFrom::Start(position))?;
        let count = file.read(&mut buffer[..length])?;
        if count == 0 {
            return Ok(None);
        }
        if let Some(index) = buffer[..count]
            .windows(2)
            .position(|window| window == MAGIC)
        {
            return Ok(Some(position + index as u64));
        }
        if count < MAGIC.len() {
            break;
        }
        position += (count - (MAGIC.len() - 1)) as u64;
    }
    Ok(None)
}

// Original: packages/minidb/src/codec.ts, scanFrameRefsFd().
pub fn scan_frame_refs(
    file: &mut File,
    mode: CorruptionMode,
    start_offset: u64,
) -> Result<ScanFrameRefsResult, CodecError> {
    let size = file.metadata()?.len();
    let mut frames = Vec::new();
    let mut corrupt_ranges = Vec::new();
    let mut position = start_offset;
    while position < size {
        if let Some(frame) = read_frame_ref_at(file, position, size)? {
            position += frame.frame_len;
            frames.push(frame);
            continue;
        }
        if mode == CorruptionMode::Strict {
            corrupt_ranges.push((position, size));
            break;
        }
        let bad_start = position;
        let mut scan = position + 1;
        let mut resume = None;
        while scan + 1 < size {
            let Some(candidate) = find_magic(file, scan, size)? else {
                break;
            };
            if read_frame_ref_at(file, candidate, size)?.is_some() {
                resume = Some(candidate);
                break;
            }
            scan = candidate + 1;
        }
        let end = resume.unwrap_or(size);
        corrupt_ranges.push((bad_start, end));
        let Some(resume) = resume else {
            break;
        };
        position = resume;
    }
    Ok(ScanFrameRefsResult {
        frames,
        corrupt_ranges,
        eof_offset: position,
    })
}

pub fn scan_frame_refs_file(
    path: impl AsRef<Path>,
    mode: CorruptionMode,
) -> Result<ScanFrameRefsResult, CodecError> {
    scan_frame_refs(&mut File::open(path)?, mode, 0)
}

// Original: packages/minidb/src/codec.ts, scanBatchOpRefs().
pub fn scan_batch_op_refs(body: &[u8], body_offset: u64) -> Result<Vec<BatchOpRef>, CodecError> {
    if body.len() < 2 {
        return Ok(Vec::new());
    }
    let count = u16::from_le_bytes(body[..2].try_into().expect("count"));
    let mut offset = 2;
    let mut operations = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let header = body
            .get(offset..offset + SUB_HEADER_SIZE)
            .ok_or(CodecError::BatchHeaderTruncated)?;
        let op_type = header[0];
        let key_len = u16::from_le_bytes(header[1..3].try_into().expect("key length")) as usize;
        let value_len = u32::from_le_bytes(header[3..7].try_into().expect("value length"));
        let meta_len = u32::from_le_bytes(header[7..11].try_into().expect("meta length")) as usize;
        let expire_at = i64::from_le_bytes(header[11..19].try_into().expect("expiration"));
        offset += SUB_HEADER_SIZE;
        let end = offset
            .checked_add(key_len)
            .and_then(|end| end.checked_add(value_len as usize))
            .and_then(|end| end.checked_add(meta_len))
            .filter(|end| *end <= body.len())
            .ok_or(CodecError::BatchPayloadTruncated)?;
        let key = body[offset..offset + key_len].to_vec();
        let value_offset = body_offset + offset as u64 + key_len as u64;
        offset += key_len + value_len as usize;
        let meta = (meta_len > 0).then(|| body[offset..offset + meta_len].to_vec());
        offset = end;
        operations.push(BatchOpRef {
            op_type,
            key,
            meta,
            expire_at,
            value_offset,
            value_len,
        });
    }
    Ok(operations)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_frame(key: &str, value: &str) -> Frame {
        Frame {
            frame_type: TYPE_SET,
            key: key.as_bytes().to_vec(),
            value: value.as_bytes().to_vec(),
            meta: None,
            expire_at: 123,
        }
    }

    #[test]
    fn frames_round_trip_across_chunks_and_report_torn_tail() {
        let first = encode_frame(&set_frame("foo", "bar")).unwrap();
        assert_eq!(first.len(), HEADER_SIZE + 3 + 3 + CRC_SIZE);
        let mut parser = FrameParser::new();
        assert!(parser.feed(&first[..7]).unwrap().is_empty());
        assert_eq!(
            parser.feed(&first[7..]).unwrap(),
            vec![set_frame("foo", "bar")]
        );
        assert_eq!(parser.finish().unwrap(), first.len() as u64);

        let mut parser = FrameParser::new();
        parser.feed(&first).unwrap();
        parser.feed(&MAGIC).unwrap();
        assert_eq!(
            parser.finish().unwrap_err().corrupt_offset(),
            Some(first.len() as u64)
        );
    }

    #[test]
    fn batch_and_resync_preserve_atomic_payloads_and_later_frames() {
        let operations = vec![BatchOp {
            op_type: TYPE_SET,
            key: b"a".to_vec(),
            value: Some(b"1".to_vec()),
            meta: Some(b"m".to_vec()),
            expire_at: 5,
        }];
        let body = encode_batch_ops(&operations).unwrap();
        assert_eq!(decode_batch_ops(&body).unwrap(), operations);

        let first = encode_frame(&set_frame("a", "1")).unwrap();
        let second = encode_frame(&set_frame("b", "2")).unwrap();
        let mut bytes = first.clone();
        bytes.extend_from_slice(b"broken");
        bytes.extend_from_slice(&second);
        let parsed = parse_buffer(&bytes, CorruptionMode::Resync);
        assert_eq!(parsed.frames.len(), 2);
        assert_eq!(
            parsed.corrupt_ranges,
            vec![(first.len() as u64, (first.len() + 6) as u64)]
        );
    }
}
