use sha2::{Digest, Sha256};

pub fn encode_hex(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(bytes)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    encode_hex(Sha256::digest(bytes))
}

pub fn sha256_hex_prefix(bytes: &[u8], digest_bytes: usize) -> String {
    let digest = Sha256::digest(bytes);
    encode_hex(&digest[..digest_bytes.min(digest.len())])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_bytes_and_sha256_with_lowercase_hex() {
        assert_eq!(encode_hex([0x00, 0xab, 0xff]), "00abff");
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(sha256_hex_prefix(b"abc", 4), "ba7816bf");
    }
}
