use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Mutex,
};

use thiserror::Error;

use crate::store::{ValueFile, ValueLoc};

#[derive(Debug, Error)]
pub enum ValueReaderError {
    #[error("value reader: {0} file is not open")]
    NotOpen(&'static str),
    #[error("value reader: short read from {file} at {offset}")]
    ShortRead { file: &'static str, offset: u64 },
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("value reader lock is poisoned")]
    Poisoned,
}

pub struct PositionedValueReader {
    pub snapshot_path: PathBuf,
    pub wal_path: PathBuf,
    snapshot: Mutex<Option<File>>,
    wal: Mutex<Option<File>>,
}

impl PositionedValueReader {
    pub fn new(directory: impl AsRef<Path>) -> Self {
        Self {
            snapshot_path: directory.as_ref().join("db.snapshot"),
            wal_path: directory.as_ref().join("db.wal"),
            snapshot: Mutex::new(None),
            wal: Mutex::new(None),
        }
    }

    // Original: packages/minidb/src/value-reader.ts, ValueReader.open().
    pub fn open(&self) -> Result<(), ValueReaderError> {
        *self
            .snapshot
            .lock()
            .map_err(|_| ValueReaderError::Poisoned)? = open_if_exists(&self.snapshot_path)?;
        *self.wal.lock().map_err(|_| ValueReaderError::Poisoned)? = open_if_exists(&self.wal_path)?;
        Ok(())
    }

    pub fn read(&self, location: ValueLoc) -> Result<Vec<u8>, ValueReaderError> {
        if location.len == 0 {
            return Ok(Vec::new());
        }
        let (slot, name) = match location.file {
            ValueFile::Snapshot => (&self.snapshot, "snapshot"),
            ValueFile::Wal => (&self.wal, "wal"),
        };
        let mut guard = slot.lock().map_err(|_| ValueReaderError::Poisoned)?;
        let file = guard.as_mut().ok_or(ValueReaderError::NotOpen(name))?;
        file.seek(SeekFrom::Start(location.offset))?;
        let mut output = vec![0; location.len as usize];
        let mut read = 0;
        while read < output.len() {
            let count = file.read(&mut output[read..])?;
            if count == 0 {
                return Err(ValueReaderError::ShortRead {
                    file: name,
                    offset: location.offset + read as u64,
                });
            }
            read += count;
        }
        Ok(output)
    }

    pub fn reopen_snapshot(&self) -> Result<(), ValueReaderError> {
        *self
            .snapshot
            .lock()
            .map_err(|_| ValueReaderError::Poisoned)? = open_if_exists(&self.snapshot_path)?;
        Ok(())
    }

    pub fn reopen_wal(&self) -> Result<(), ValueReaderError> {
        *self.wal.lock().map_err(|_| ValueReaderError::Poisoned)? = open_if_exists(&self.wal_path)?;
        Ok(())
    }

    pub fn reopen_both(&self) -> Result<(), ValueReaderError> {
        self.reopen_snapshot()?;
        self.reopen_wal()
    }

    pub fn close(&self) -> Result<(), ValueReaderError> {
        *self
            .snapshot
            .lock()
            .map_err(|_| ValueReaderError::Poisoned)? = None;
        *self.wal.lock().map_err(|_| ValueReaderError::Poisoned)? = None;
        Ok(())
    }
}

fn open_if_exists(path: &Path) -> io::Result<Option<File>> {
    match OpenOptions::new().read(true).open(path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn reads_positioned_values_and_reopens_rotated_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("db.wal");
        File::create(&path)
            .unwrap()
            .write_all(b"prefix-value")
            .unwrap();
        let reader = PositionedValueReader::new(directory.path());
        reader.open().unwrap();
        assert_eq!(
            reader
                .read(ValueLoc {
                    file: ValueFile::Wal,
                    offset: 7,
                    len: 5
                })
                .unwrap(),
            b"value"
        );
        reader.close().unwrap();
        assert!(matches!(
            reader.read(ValueLoc {
                file: ValueFile::Wal,
                offset: 0,
                len: 1
            }),
            Err(ValueReaderError::NotOpen("wal"))
        ));
    }
}
