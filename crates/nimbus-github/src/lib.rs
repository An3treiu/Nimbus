use base64::{engine::general_purpose::STANDARD, Engine};

/// Encode raw bytes the way GitHub's create-blob endpoint expects.
pub fn encode_blob(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// Decode the base64 content GitHub returns from get-blob.
/// GitHub wraps lines at 60 chars, so whitespace must be stripped first.
pub fn decode_blob(content: &str) -> Result<Vec<u8>, base64::DecodeError> {
    let cleaned: String = content.chars().filter(|c| !c.is_whitespace()).collect();
    STANDARD.decode(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_bytes() {
        let data = b"hello nimbus \xff\x00";
        let encoded = encode_blob(data);
        let decoded = decode_blob(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn decode_tolerates_github_line_wrapping() {
        // GitHub returns base64 with embedded newlines.
        let encoded = encode_blob(b"the quick brown fox jumps over the lazy dog");
        let wrapped = format!("{}\n{}", &encoded[..8], &encoded[8..]);
        let decoded = decode_blob(&wrapped).unwrap();
        assert_eq!(decoded, b"the quick brown fox jumps over the lazy dog");
    }
}
