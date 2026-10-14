//! Standard base64 (RFC 4648 section 4) with padding, for update keys and
//! signatures. Decoding is strict: no whitespace inside, no missing
//! padding, and the unused bits of the last character must be zero, so
//! every byte string has exactly one accepted encoding.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encodes `bytes` with padding.
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let symbols = chunk.len() + 1;
        for i in 0..4 {
            if i < symbols {
                let index = ((n >> (18 - 6 * i)) & 0x3f) as usize;
                out.push(char::from(ALPHABET[index]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn value(c: u8) -> Option<u32> {
    ALPHABET.iter().position(|&a| a == c).map(|p| p as u32)
}

/// Decodes padded standard base64, or `None` for anything else.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let chunks = bytes.len() / 4;
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let last = index + 1 == chunks;
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for &c in &chunk[..4 - pad] {
            n = (n << 6) | value(c)?;
        }
        n <<= 6 * pad as u32;
        let decoded = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        let keep = 3 - pad;
        // The bits below the kept bytes must be zero.
        if decoded[keep..].iter().any(|&b| b != 0) {
            return None;
        }
        out.extend_from_slice(&decoded[..keep]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTORS: [(&str, &str); 7] = [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ];

    #[test]
    fn rfc_4648_vectors() {
        for (plain, encoded) in VECTORS {
            assert_eq!(encode(plain.as_bytes()), encoded);
            assert_eq!(decode(encoded).as_deref(), Some(plain.as_bytes()));
        }
    }

    #[test]
    fn round_trips_every_byte() {
        let bytes: Vec<u8> = (0..=255).collect();
        for len in [0, 1, 2, 3, 31, 32, 64, 256] {
            let encoded = encode(&bytes[..len]);
            assert_eq!(decode(&encoded).as_deref(), Some(&bytes[..len]));
        }
        // The two symbols that differ between the standard and URL alphabets.
        assert_eq!(encode(&[0xfb, 0xff, 0xbf]), "+/+/");
        assert_eq!(decode("+/+/").as_deref(), Some(&[0xfb, 0xff, 0xbf][..]));
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in [
            "Zg",       // missing padding
            "Zg=",      // short padding
            "Z===",     // too much padding
            "Zg==Zg==", // padding before the end
            "Zm9v\n",   // whitespace
            "Zm 9",     // space
            "Zm9-",     // URL-safe alphabet
            "Zh==",     // nonzero unused bits
            "Zm9=",     // nonzero unused bits
            "====",
        ] {
            assert_eq!(decode(bad), None, "{bad:?}");
        }
    }
}
