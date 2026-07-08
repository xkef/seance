//! iTerm2 inline-image protocol (`OSC 1337 ; File = …`) → Kitty graphics.
//!
//! seance renders images through the Kitty graphics stack. Rather than grow a
//! second image pipeline, an iTerm2 inline image is translated into a synthetic
//! Kitty transmit-and-display command that the libghostty parser already
//! understands. The envelope is:
//!
//! ```text
//! ESC ] 1337 ; File = <key=value>;<key=value>… : <base64-payload> ST
//! ```
//!
//! Only `inline=1` payloads display; the download variant (`inline=0`, absent)
//! is left untranslated. The payload must be PNG — that is the only encoding
//! the Kitty `f=100` transmission path decodes here. Non-PNG images (JPEG, GIF)
//! are dropped rather than mis-rendered; supporting them would require decoding
//! to raw RGBA on this side, which is out of scope for the translator.

/// Kitty limits each transmission chunk's base64 payload to 4096 bytes.
const KITTY_CHUNK: usize = 4096;

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Translate the body of an `OSC 1337 ; File=…` sequence (the bytes between
/// `OSC` and the string terminator) into a Kitty graphics APC byte sequence
/// that transmits and displays the image at the cursor.
///
/// Returns `None` when the sequence is not a displayable inline PNG image, in
/// which case the caller leaves the original bytes untranslated.
pub(crate) fn osc1337_to_kitty(content: &[u8]) -> Option<Vec<u8>> {
    let rest = content.strip_prefix(b"1337;File=")?;
    let colon = rest.iter().position(|&b| b == b':')?;
    let (args, payload) = rest.split_at(colon);
    let payload = &payload[1..];

    let params = FileParams::parse(args);
    if !params.inline {
        return None;
    }

    let b64: Vec<u8> = payload
        .iter()
        .copied()
        .filter(|b| !matches!(b, b'\r' | b'\n' | b'\t' | b' '))
        .collect();
    let decoded = crate::clipboard::base64_decode(&b64)?;
    if !decoded.starts_with(PNG_MAGIC) {
        return None;
    }

    Some(encode_kitty(&b64, params.columns, params.rows))
}

#[derive(Default)]
struct FileParams {
    inline: bool,
    columns: Option<u32>,
    rows: Option<u32>,
}

impl FileParams {
    fn parse(args: &[u8]) -> Self {
        let mut params = FileParams::default();
        for pair in args.split(|&b| b == b';') {
            let Some(eq) = pair.iter().position(|&b| b == b'=') else {
                continue;
            };
            let (key, value) = pair.split_at(eq);
            let value = &value[1..];
            match key {
                b"inline" => params.inline = value == b"1",
                b"width" => params.columns = parse_cell_dimension(value),
                b"height" => params.rows = parse_cell_dimension(value),
                _ => {}
            }
        }
        params
    }
}

/// iTerm2 dimensions may be a bare cell count (`N`), pixels (`Npx`), a
/// percentage (`N%`), or `auto`. Kitty placement geometry is expressed in
/// cells, so only the bare cell-count form maps cleanly; every other form
/// falls back to the image's natural size.
fn parse_cell_dimension(value: &[u8]) -> Option<u32> {
    let text = std::str::from_utf8(value).ok()?;
    text.parse::<u32>().ok().filter(|&n| n > 0)
}

fn encode_kitty(b64: &[u8], columns: Option<u32>, rows: Option<u32>) -> Vec<u8> {
    let chunks: Vec<&[u8]> = if b64.is_empty() {
        vec![&b64[..]]
    } else {
        b64.chunks(KITTY_CHUNK).collect()
    };

    let mut out = Vec::with_capacity(b64.len() + chunks.len() * 24);
    for (index, chunk) in chunks.iter().enumerate() {
        let first = index == 0;
        let last = index == chunks.len() - 1;
        out.extend_from_slice(b"\x1b_G");
        if first {
            out.extend_from_slice(b"a=T,f=100,q=2");
            if let Some(c) = columns {
                out.extend_from_slice(format!(",c={c}").as_bytes());
            }
            if let Some(r) = rows {
                out.extend_from_slice(format!(",r={r}").as_bytes());
            }
            if !last {
                out.extend_from_slice(b",m=1");
            }
        } else {
            out.extend_from_slice(if last { b"m=0" } else { b"m=1" });
        }
        out.push(b';');
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2×2 RGBA PNG (red, green / blue, yellow).
    const PNG_2X2: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAFElEQVR4nGP4z8DwHwyBNBAw/AcAR8oI+ItOQ4UAAAAASUVORK5CYII=";

    fn osc1337(args: &str, b64: &str) -> Vec<u8> {
        format!("1337;File={args}:{b64}").into_bytes()
    }

    #[test]
    fn translates_inline_png_to_transmit_and_display() {
        let content = osc1337("inline=1", PNG_2X2);
        let apc = osc1337_to_kitty(&content).expect("translation");
        assert!(apc.starts_with(b"\x1b_Ga=T,f=100,q=2;"));
        assert!(apc.ends_with(b"\x1b\\"));
        // Single small chunk carries the exact base64 the app sent.
        let body = &apc[apc.iter().position(|&b| b == b';').unwrap() + 1..apc.len() - 2];
        assert_eq!(body, PNG_2X2.as_bytes());
    }

    #[test]
    fn maps_width_and_height_to_columns_and_rows() {
        let content = osc1337("inline=1;width=10;height=4", PNG_2X2);
        let apc = osc1337_to_kitty(&content).expect("translation");
        let header = &apc[..apc.iter().position(|&b| b == b';').unwrap()];
        assert_eq!(header, b"\x1b_Ga=T,f=100,q=2,c=10,r=4");
    }

    #[test]
    fn ignores_non_cell_dimensions() {
        let content = osc1337("inline=1;width=80px;height=50%", PNG_2X2);
        let apc = osc1337_to_kitty(&content).expect("translation");
        let header = &apc[..apc.iter().position(|&b| b == b';').unwrap()];
        assert_eq!(header, b"\x1b_Ga=T,f=100,q=2");
    }

    #[test]
    fn drops_download_variant() {
        assert!(osc1337_to_kitty(&osc1337("inline=0", PNG_2X2)).is_none());
        assert!(osc1337_to_kitty(&osc1337("name=Zg==", PNG_2X2)).is_none());
    }

    #[test]
    fn drops_non_png_payload() {
        // JPEG magic (base64 of 0xFF 0xD8 0xFF).
        assert!(osc1337_to_kitty(&osc1337("inline=1", "/9j/")).is_none());
    }

    #[test]
    fn drops_non_1337_osc() {
        assert!(osc1337_to_kitty(b"52;c;aGVsbG8=").is_none());
    }

    #[test]
    fn chunks_large_payloads_with_continuation_flags() {
        // A payload longer than one Kitty chunk must split with m=1 … m=0.
        let big = "A".repeat(KITTY_CHUNK * 2 + 8);
        let apc = encode_kitty(big.as_bytes(), None, None);
        let text = String::from_utf8(apc).unwrap();
        let parts: Vec<&str> = text.split("\x1b\\").filter(|s| !s.is_empty()).collect();
        assert_eq!(parts.len(), 3);
        assert!(parts[0].starts_with("\x1b_Ga=T,f=100,q=2,m=1;"));
        assert!(parts[1].starts_with("\x1b_Gm=1;"));
        assert!(parts[2].starts_with("\x1b_Gm=0;"));
    }
}
