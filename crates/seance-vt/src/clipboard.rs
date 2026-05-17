//! OSC 52 clipboard request parsing.
//!
//! OSC 52 (xterm) lets the application drive the host's clipboard:
//!
//! - `ESC ] 52 ; <sel> ; <base64> BEL` — set the clipboard.
//! - `ESC ] 52 ; <sel> ; ? BEL` — query the clipboard.
//!
//! `<sel>` is any combination of `c` (clipboard), `p`/`s` (primary/selection),
//! `q` (secondary), and `0`–`7` (cut buffers). seance maps every recognized
//! selector onto the OS clipboard — the host platforms we target (macOS,
//! Wayland with arboard fallbacks) only expose a single clipboard, and tmux
//! / vim drivers always send `c` anyway.
//!
//! The decoded [`ClipboardRequest`] data type and the `encode_osc52_reply`
//! helper live in [`seance_protocol::clipboard`]; only the parser is here.

pub use seance_protocol::clipboard::{ClipboardRequest, encode_osc52_reply};

/// Parse the body of an OSC 52 command (i.e. the bytes between `OSC 52 ;` and
/// the terminator). Returns `None` for malformed payloads — silently dropping
/// is the xterm-compatible behavior.
pub(crate) fn parse_osc52(content: &[u8]) -> Option<ClipboardRequest> {
    let body = content.strip_prefix(b"52;")?;
    let semi = body.iter().position(|&b| b == b';')?;
    let (_selectors, rest) = body.split_at(semi);
    let payload = &rest[1..];
    if payload == b"?" {
        return Some(ClipboardRequest::Read);
    }
    let decoded = base64_decode(payload)?;
    Some(ClipboardRequest::Write(decoded))
}

fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    let mut buf = Vec::with_capacity(input.len() * 3 / 4);
    let mut group = 0u32;
    let mut filled = 0u8;
    let mut pad = 0u8;
    for &byte in input {
        // Whitespace is permitted between groups (some shells fold long
        // OSC 52 payloads).
        if matches!(byte, b'\r' | b'\n' | b'\t' | b' ') {
            continue;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => {
                pad = pad.saturating_add(1);
                if pad > 2 {
                    return None;
                }
                group <<= 6;
                filled += 1;
                if filled == 4 {
                    // 1 pad encodes 2 data bytes, 2 pads encode 1 data byte.
                    flush_group(group, 3 - pad, &mut buf);
                    group = 0;
                    filled = 0;
                }
                continue;
            }
            _ => return None,
        };
        if pad != 0 {
            return None;
        }
        group = (group << 6) | u32::from(value);
        filled += 1;
        if filled == 4 {
            flush_group(group, 3, &mut buf);
            group = 0;
            filled = 0;
        }
    }
    if filled != 0 {
        return None;
    }
    Some(buf)
}

fn flush_group(group: u32, take: u8, out: &mut Vec<u8>) {
    let bytes = [(group >> 16) as u8, (group >> 8) as u8, group as u8];
    for byte in bytes.iter().take(take as usize) {
        out.push(*byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_write_with_clipboard_selector() {
        let req = parse_osc52(b"52;c;aGVsbG8=").expect("parse");
        assert_eq!(req, ClipboardRequest::Write(b"hello".to_vec()));
    }

    #[test]
    fn parses_write_with_multiple_selectors() {
        let req = parse_osc52(b"52;cps;aGVsbG8=").expect("parse");
        assert_eq!(req, ClipboardRequest::Write(b"hello".to_vec()));
    }

    #[test]
    fn parses_write_with_empty_selector() {
        let req = parse_osc52(b"52;;aGVsbG8=").expect("parse");
        assert_eq!(req, ClipboardRequest::Write(b"hello".to_vec()));
    }

    #[test]
    fn parses_read_request() {
        assert_eq!(parse_osc52(b"52;c;?"), Some(ClipboardRequest::Read));
    }

    #[test]
    fn rejects_missing_payload_separator() {
        assert_eq!(parse_osc52(b"52;c"), None);
    }

    #[test]
    fn rejects_non_osc52_commands() {
        assert_eq!(parse_osc52(b"7;file:///tmp"), None);
    }

    #[test]
    fn rejects_invalid_base64() {
        assert_eq!(parse_osc52(b"52;c;@@@@"), None);
    }

    #[test]
    fn round_trips_known_payloads() {
        for payload in [
            &b""[..],
            b"a",
            b"ab",
            b"abc",
            b"abcd",
            b"hello, world!",
            b"\x00\x01\x02\x03\x04",
        ] {
            let encoded = encode_osc52_reply(payload);
            let body = encoded
                .strip_prefix(b"\x1b]52;c;")
                .and_then(|rest| rest.strip_suffix(b"\x1b\\"))
                .expect("reply framing");
            let mut osc = Vec::from(b"52;c;");
            osc.extend_from_slice(body);
            let req = parse_osc52(&osc).expect("parse encoded reply");
            assert_eq!(req, ClipboardRequest::Write(payload.to_vec()));
        }
    }

    #[test]
    fn decodes_folded_base64_payload() {
        let req = parse_osc52(b"52;c;aGVs\nbG8=").expect("parse");
        assert_eq!(req, ClipboardRequest::Write(b"hello".to_vec()));
    }
}
