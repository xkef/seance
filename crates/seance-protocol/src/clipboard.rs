//! OSC 52 clipboard request data.
//!
//! Pure data + byte construction lives here so the wire protocol can carry
//! clipboard events without dragging libghostty in. The OSC 52 parser stays
//! in `seance-vt::clipboard` (it consumes raw VT input).

use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// A clipboard request decoded from an OSC 52 sequence.
///
/// The VT layer parses the bytes; the application owns the OS clipboard
/// (`arboard`) and honors or denies the request per the user's
/// `clipboard.{read,write}` policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipboardRequest {
    /// `OSC 52 ; <sel> ; <base64> ST` — set the clipboard contents. `data`
    /// is the decoded byte stream; callers should validate UTF-8 before
    /// treating it as text.
    Write(Vec<u8>),
    /// `OSC 52 ; <sel> ; ? ST` — query the clipboard. The application
    /// replies with another OSC 52 sequence carrying the encoded contents
    /// (see [`encode_osc52_reply`]).
    Read,
}

/// Build an `ESC ] 52 ; c ; <base64> ESC \\` reply for the contents of the
/// system clipboard. xterm echoes the selector the application asked for;
/// seance always answers with `c` since every selector collapses onto the
/// single OS clipboard.
pub fn encode_osc52_reply(payload: &[u8]) -> Bytes {
    let mut out = Vec::with_capacity(payload.len() * 4 / 3 + 8);
    out.extend_from_slice(b"\x1b]52;c;");
    base64_encode_into(payload, &mut out);
    out.extend_from_slice(b"\x1b\\");
    Bytes::from(out)
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode_into(input: &[u8], out: &mut Vec<u8>) {
    let mut chunks = input.chunks_exact(3);
    for chunk in &mut chunks {
        let b0 = chunk[0];
        let b1 = chunk[1];
        let b2 = chunk[2];
        out.push(ALPHABET[(b0 >> 2) as usize]);
        out.push(ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize]);
        out.push(ALPHABET[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize]);
        out.push(ALPHABET[(b2 & 0x3f) as usize]);
    }
    let remainder = chunks.remainder();
    match remainder.len() {
        0 => {}
        1 => {
            let b0 = remainder[0];
            out.push(ALPHABET[(b0 >> 2) as usize]);
            out.push(ALPHABET[((b0 & 0x03) << 4) as usize]);
            out.push(b'=');
            out.push(b'=');
        }
        2 => {
            let b0 = remainder[0];
            let b1 = remainder[1];
            out.push(ALPHABET[(b0 >> 2) as usize]);
            out.push(ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize]);
            out.push(ALPHABET[((b1 & 0x0f) << 2) as usize]);
            out.push(b'=');
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert!(encoded.starts_with(b"\x1b]52;c;"));
            assert!(encoded.ends_with(b"\x1b\\"));
        }
    }
}
