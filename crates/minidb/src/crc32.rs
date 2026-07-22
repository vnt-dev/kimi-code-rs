use std::sync::OnceLock;

const POLYNOMIAL: u32 = 0xedb8_8320;
static TABLE: OnceLock<[u32; 256]> = OnceLock::new();

fn build_table() -> [u32; 256] {
    let mut table = [0; 256];
    for (index, entry) in table.iter_mut().enumerate() {
        let mut crc = index as u32;
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                POLYNOMIAL ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
        *entry = crc;
    }
    table
}

// Original: packages/minidb/src/crc32.ts, crc32().
pub fn crc32(bytes: &[u8], previous: u32) -> u32 {
    let table = TABLE.get_or_init(build_table);
    let mut crc = previous ^ u32::MAX;
    for byte in bytes {
        crc = table[((crc ^ u32::from(*byte)) & 0xff) as usize] ^ (crc >> 8);
    }
    crc ^ u32::MAX
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
