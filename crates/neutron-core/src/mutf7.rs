//! IMAP mailbox-name encoding: modified UTF-7 (RFC 3501 §5.1.3), used
//! unless the server accepted `ENABLE UTF8=ACCEPT` (RFC 6855).

use base64::alphabet::IMAP_MUTF7;
use base64::engine::general_purpose::{GeneralPurpose, NO_PAD};
use base64::Engine as _;

const B64: GeneralPurpose = GeneralPurpose::new(&IMAP_MUTF7, NO_PAD);

/// Encode a Unicode mailbox name for the wire.
pub fn encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending: Vec<u16> = Vec::new();
    let flush = |pending: &mut Vec<u16>, out: &mut String| {
        if pending.is_empty() {
            return;
        }
        let bytes: Vec<u8> = pending.iter().flat_map(|u| u.to_be_bytes()).collect();
        out.push('&');
        out.push_str(&B64.encode(bytes));
        out.push('-');
        pending.clear();
    };
    for c in name.chars() {
        if (' '..='~').contains(&c) {
            flush(&mut pending, &mut out);
            if c == '&' {
                out.push_str("&-");
            } else {
                out.push(c);
            }
        } else {
            let mut buf = [0u16; 2];
            pending.extend_from_slice(c.encode_utf16(&mut buf));
        }
    }
    flush(&mut pending, &mut out);
    out
}

/// Decode a wire name. Invalid sequences are kept verbatim (a name we can't
/// decode must still round-trip to the server unchanged, so callers keep
/// the raw name for commands and use this only for display). Raw UTF-8
/// (UTF8=ACCEPT servers) passes through.
pub fn decode(wire: &str) -> String {
    let mut out = String::with_capacity(wire.len());
    let mut rest = wire;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(end) = after.find('-') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let chunk = &after[..end];
        if chunk.is_empty() {
            out.push('&');
        } else {
            match B64
                .decode(chunk)
                .ok()
                .filter(|b| b.len() % 2 == 0)
                .and_then(|b| {
                    let units: Vec<u16> = b
                        .chunks(2)
                        .map(|p| u16::from_be_bytes([p[0], p[1]]))
                        .collect();
                    String::from_utf16(&units).ok()
                }) {
                Some(s) => out.push_str(&s),
                None => out.push_str(&rest[i..i + 1 + end + 1]),
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3501_examples_and_round_trips() {
        // RFC 3501 §5.1.3 example.
        assert_eq!(
            encode("~peter/mail/台北/日本語"),
            "~peter/mail/&U,BTFw-/&ZeVnLIqe-"
        );
        assert_eq!(
            decode("~peter/mail/&U,BTFw-/&ZeVnLIqe-"),
            "~peter/mail/台北/日本語"
        );
        assert_eq!(encode("Tom & Jerry"), "Tom &- Jerry");
        assert_eq!(decode("Tom &- Jerry"), "Tom & Jerry");
        for s in [
            "收件箱/项目",
            "Größe",
            "😀 emoji",
            "INBOX",
            "a&b&",
            "Ελληνικά.Δοκιμή",
        ] {
            assert_eq!(decode(&encode(s)), s, "{s}");
            assert!(encode(s).is_ascii());
        }
        // Already-UTF-8 names (UTF8=ACCEPT) and garbage pass through.
        assert_eq!(decode("项目"), "项目");
        assert_eq!(decode("&bad"), "&bad");
        assert_eq!(decode("x&!!!-y"), "x&!!!-y");
    }
}
