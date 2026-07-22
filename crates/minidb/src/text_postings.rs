use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::crc32::crc32;

const HEADER_LEN: usize = 10;
const CRC_LEN: usize = 4;

#[derive(Debug, Error)]
pub enum PostingsError {
    #[error("postings: truncated varint")]
    TruncatedVarint,
    #[error("postings: varint too long")]
    VarintTooLong,
    #[error("postings: term too long")]
    TermTooLong,
    #[error("postings: record too short")]
    RecordTooShort,
    #[error("postings: record crc mismatch")]
    CrcMismatch,
    #[error("postings: record term length out of bounds")]
    TermOutOfBounds,
    #[error("postings: record payload length out of bounds")]
    PayloadOutOfBounds,
    #[error("postings file is closed")]
    Closed,
    #[error("postings: rebuild write made no progress")]
    NoWriteProgress,
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedRecord {
    pub term: String,
    pub document_frequency: u32,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PostingEntry {
    pub offset: u64,
    pub len: u32,
    pub document_frequency: u32,
}

// Original: packages/minidb/src/text-postings.ts, encodePostingList().
pub fn encode_posting_list(entries: &[(u32, u32)]) -> Vec<u8> {
    let mut output = Vec::new();
    encode_varint(entries.len() as u32, &mut output);
    let mut previous = 0_u32;
    for &(document_id, frequency) in entries {
        encode_varint(document_id.wrapping_sub(previous), &mut output);
        encode_varint(frequency, &mut output);
        previous = document_id;
    }
    output
}

pub fn decode_posting_list(bytes: &[u8]) -> Result<Vec<(u32, u32)>, PostingsError> {
    let mut cursor = 0;
    let count = decode_varint(bytes, &mut cursor)?;
    let mut output = Vec::with_capacity(count as usize);
    let mut previous = 0_u32;
    for _ in 0..count {
        previous = previous.wrapping_add(decode_varint(bytes, &mut cursor)?);
        output.push((previous, decode_varint(bytes, &mut cursor)?));
    }
    Ok(output)
}

fn encode_varint(mut value: u32, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn decode_varint(bytes: &[u8], cursor: &mut usize) -> Result<u32, PostingsError> {
    let mut result = 0_u32;
    let mut shift = 0;
    loop {
        let byte = *bytes.get(*cursor).ok_or(PostingsError::TruncatedVarint)?;
        *cursor += 1;
        if shift < 32 {
            result |= ((byte & 0x7f) as u32) << shift;
        }
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift > 35 {
            return Err(PostingsError::VarintTooLong);
        }
    }
}

pub fn encode_record(
    term: &str,
    document_frequency: u32,
    payload: &[u8],
) -> Result<Vec<u8>, PostingsError> {
    let term = term.as_bytes();
    let term_len = u16::try_from(term.len()).map_err(|_| PostingsError::TermTooLong)?;
    let payload_len =
        u32::try_from(payload.len()).map_err(|_| PostingsError::PayloadOutOfBounds)?;
    let mut output = Vec::with_capacity(HEADER_LEN + term.len() + payload.len() + CRC_LEN);
    output.extend_from_slice(&term_len.to_le_bytes());
    output.extend_from_slice(term);
    output.extend_from_slice(&document_frequency.to_le_bytes());
    output.extend_from_slice(&payload_len.to_le_bytes());
    output.extend_from_slice(payload);
    let checksum = crc32(&output, 0);
    output.extend_from_slice(&checksum.to_le_bytes());
    Ok(output)
}

pub fn decode_record(bytes: &[u8]) -> Result<DecodedRecord, PostingsError> {
    if bytes.len() < HEADER_LEN + CRC_LEN {
        return Err(PostingsError::RecordTooShort);
    }
    let body_len = bytes.len() - CRC_LEN;
    let stored = u32::from_le_bytes(bytes[body_len..].try_into().expect("crc trailer"));
    if stored != crc32(&bytes[..body_len], 0) {
        return Err(PostingsError::CrcMismatch);
    }
    let term_len = u16::from_le_bytes(bytes[..2].try_into().expect("term length")) as usize;
    let after_term = 2 + term_len;
    if after_term + 8 > body_len {
        return Err(PostingsError::TermOutOfBounds);
    }
    let term = String::from_utf8_lossy(&bytes[2..after_term]).into_owned();
    let document_frequency =
        u32::from_le_bytes(bytes[after_term..after_term + 4].try_into().expect("df"));
    let payload_len = u32::from_le_bytes(
        bytes[after_term + 4..after_term + 8]
            .try_into()
            .expect("payload length"),
    ) as usize;
    let payload_start = after_term + 8;
    if payload_start + payload_len > body_len {
        return Err(PostingsError::PayloadOutOfBounds);
    }
    Ok(DecodedRecord {
        term,
        document_frequency,
        payload: bytes[payload_start..payload_start + payload_len].to_vec(),
    })
}

pub struct PostingsFile {
    pub path: PathBuf,
    file: Option<File>,
}

impl PostingsFile {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PostingsError> {
        let path = path.as_ref().to_owned();
        let file = OpenOptions::new().read(true).open(&path)?;
        Ok(Self {
            path,
            file: Some(file),
        })
    }

    pub fn is_open(&self) -> bool {
        self.file.is_some()
    }

    pub fn read(&mut self, entry: PostingEntry) -> Result<Vec<(u32, u32)>, PostingsError> {
        let file = self.file.as_mut().ok_or(PostingsError::Closed)?;
        file.seek(SeekFrom::Start(entry.offset))?;
        let mut bytes = vec![0; entry.len as usize];
        file.read_exact(&mut bytes)?;
        decode_posting_list(&decode_record(&bytes)?.payload)
    }

    pub fn close(&mut self) {
        self.file = None;
    }

    // Original: PostingsFile.rebuildSync(). Call from a blocking boundary when used by async code.
    pub fn rebuild(
        path: impl AsRef<Path>,
        entries: impl IntoIterator<Item = (String, Vec<(u32, u32)>)>,
    ) -> Result<HashMap<String, PostingEntry>, PostingsError> {
        let path = path.as_ref();
        let temporary = PathBuf::from(format!("{}.tmp", path.display()));
        let mut file = File::create(&temporary)?;
        let mut dictionary = HashMap::new();
        let mut offset = 0_u64;
        let write_result = (|| {
            for (term, postings) in entries {
                if postings.is_empty() {
                    continue;
                }
                let record = encode_record(
                    &term,
                    postings.len() as u32,
                    &encode_posting_list(&postings),
                )?;
                let mut written = 0;
                while written < record.len() {
                    let count = file.write(&record[written..])?;
                    if count == 0 {
                        return Err(PostingsError::NoWriteProgress);
                    }
                    written += count;
                }
                dictionary.insert(
                    term,
                    PostingEntry {
                        offset,
                        len: record.len() as u32,
                        document_frequency: postings.len() as u32,
                    },
                );
                offset += record.len() as u64;
            }
            file.sync_all()?;
            Ok::<_, PostingsError>(())
        })();
        drop(file);
        write_result?;
        fs::rename(&temporary, path)?;
        if let Some(directory) = path.parent()
            && let Ok(directory) = File::open(directory)
        {
            let _ = directory.sync_all();
        }
        Ok(dictionary)
    }
}

impl Drop for PostingsFile {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posting_and_record_codecs_round_trip_and_detect_crc_damage() {
        let entries = vec![(1, 2), (5, 1), (300, 7)];
        assert_eq!(
            decode_posting_list(&encode_posting_list(&entries)).unwrap(),
            entries
        );
        let mut record = encode_record("term", 3, &encode_posting_list(&entries)).unwrap();
        assert_eq!(decode_record(&record).unwrap().term, "term");
        record[3] ^= 1;
        assert!(matches!(
            decode_record(&record),
            Err(PostingsError::CrcMismatch)
        ));
    }

    #[test]
    fn rebuilds_and_reads_postings_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("postings");
        let dictionary = PostingsFile::rebuild(&path, [("x".into(), vec![(2, 3)])]).unwrap();
        let mut file = PostingsFile::open(path).unwrap();
        assert_eq!(file.read(dictionary["x"]).unwrap(), vec![(2, 3)]);
    }
}
