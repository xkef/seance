use libghostty_vt::alloc::{Allocator, Bytes};
use libghostty_vt::kitty::graphics::{self, DecodePng, DecodedImage};

struct PngDecoder;

impl DecodePng for PngDecoder {
    fn decode_png<'alloc>(
        &mut self,
        alloc: &'alloc Allocator<'_>,
        data: &[u8],
    ) -> Option<DecodedImage<'alloc>> {
        use png::{Decoder, Transformations};

        let mut decoder = Decoder::new(std::io::Cursor::new(data));
        decoder.set_transformations(Transformations::ALPHA | Transformations::STRIP_16);

        let mut reader = decoder.read_info().ok()?;
        let buf_size = reader.output_buffer_size()?;
        let mut scratch = vec![0u8; buf_size];
        let info = reader.next_frame(&mut scratch).ok()?;

        let mut bytes = Bytes::new_with_alloc(alloc, info.buffer_size()).ok()?;
        bytes.copy_from_slice(&scratch[..info.buffer_size()]);

        Some(DecodedImage {
            width: info.width,
            height: info.height,
            data: bytes,
        })
    }
}

pub(crate) fn install_png_decoder_for_this_thread() {
    let _ = graphics::set_png_decoder(Some(Box::new(PngDecoder)));
}

#[cfg(test)]
mod tests {
    use crate::CursorShape;
    use crate::core::{DEFAULT_MAX_SCROLLBACK, VtCore, VtCoreOptions};

    #[test]
    fn vt_core_can_be_constructed_on_worker_thread() {
        std::thread::spawn(|| {
            let mut core = VtCore::new(VtCoreOptions {
                cols: 24,
                rows: 5,
                pixel_width: 240,
                pixel_height: 80,
                max_scrollback: DEFAULT_MAX_SCROLLBACK,
                initial_cursor_shape: CursorShape::Block,
            })
            .expect("worker thread should construct VT core");
            core.feed(b"\x1b[c");
        })
        .join()
        .expect("worker thread should not panic");
    }
}
