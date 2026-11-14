//! Base64 (RFC 4648, стандартный алфавит; пробелы и переводы строк пропускаются).

use crate::Error;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Декодировать; посторонний символ — ошибка с его позицией.
pub fn decode(text: &str) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3 + 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    let mut padding = false;
    for (position, c) in text.bytes().enumerate() {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => {
                padding = true;
                continue;
            }
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return Err(Error(format!("base64 data has an invalid character at position {position}"))),
        };
        if padding {
            return Err(Error(format!("base64 data continues after `=` padding at position {position}")));
        }
        acc = (acc << 6 | value as u32) & 0xff_ffff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip_and_errors() {
        for data in [&b""[..], b"f", b"fo", b"foo", b"foob", b"\x00\xff\x10\x80"] {
            assert_eq!(super::decode(&super::encode(data)).unwrap(), data);
        }
        assert_eq!(super::encode(b"foob"), "Zm9vYg==");
        assert_eq!(super::decode("Zm9v\nYg==").unwrap(), b"foob");
        assert!(super::decode("Zm9v*").unwrap_err().0.contains("position 4"));
        assert!(super::decode("Zg==Zg").is_err());
    }
}
