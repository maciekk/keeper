//! Canonical ASCII encoding of Unix path bytes. Never print raw filenames.
use anyhow::{Result, bail, ensure};

pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out
}

pub fn decode(text: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut bytes = text.bytes();
    while let Some(b) = bytes.next() {
        if b != b'\\' {
            out.push(b);
            continue;
        }
        out.push(match bytes.next() {
            Some(b'\\') => b'\\',
            Some(b't') => b'\t',
            Some(b'n') => b'\n',
            Some(b'r') => b'\r',
            Some(b'x') => {
                let a = bytes
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("short hex escape"))?;
                let b = bytes
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("short hex escape"))?;
                u8::from_str_radix(std::str::from_utf8(&[a, b])?, 16)?
            }
            _ => bail!("invalid path escape"),
        });
    }
    ensure!(encode(&out) == text, "noncanonical path encoding");
    Ok(out)
}

pub fn validate_path(path: &[u8]) -> Result<()> {
    ensure!(
        !path.is_empty() && !path.contains(&0),
        "empty path or NUL byte"
    );
    for component in path.split(|&b| b == b'/') {
        ensure!(
            !component.is_empty() && component != b"." && component != b"..",
            "path must be a normalized relative path"
        );
    }
    ensure!(
        path.split(|&b| b == b'/').next() != Some(b".keeper".as_slice()),
        "reserved .keeper path"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_all_bytes() {
        let bytes: Vec<u8> = (0..=255).collect();
        let encoded = encode(&bytes);
        assert!(encoded.is_ascii());
        assert!(!encoded.contains('\n'));
        assert_eq!(decode(&encoded).unwrap(), bytes);
    }

    #[test]
    fn reject_noncanonical_and_unsafe_paths() {
        for text in ["\\x41", "\\xFF", "\\", "\\q", "é", "\t"] {
            assert!(decode(text).is_err(), "{text:?}");
        }
        for path in ["", "/a", "a/", "a//b", "a/../b", "./a", ".keeper/x", "a\0b"] {
            assert!(validate_path(path.as_bytes()).is_err(), "{path:?}");
        }
        validate_path(b"a/.keeper/x").unwrap();
    }
}
