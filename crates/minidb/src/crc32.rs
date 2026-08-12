// Original: packages/minidb/src/crc32.ts, crc32().
pub fn crc32(bytes: &[u8], previous: u32) -> u32 {
    let mut hasher = crc32fast::Hasher::new_with_initial(previous);
    hasher.update(bytes);
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_canonical_and_incremental_vectors() {
        assert_eq!(crc32(b"123456789", 0), 0xcbf4_3926);
        assert_eq!(crc32(b"", 0), 0);
        assert_eq!(crc32(b"a", 0), 0xe8b7_be43);

        let data = b"hello world from minidb";
        let running = data
            .chunks(5)
            .fold(0, |previous, chunk| crc32(chunk, previous));
        assert_eq!(running, crc32(data, 0));
    }
}
