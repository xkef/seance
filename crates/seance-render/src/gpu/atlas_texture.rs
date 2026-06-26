//! Atlas plane upload + placeholder-view helpers. Shared by the grayscale
//! and color glyph atlases; the plane-texture slot is owned by `GpuState`.

use wgpu::*;

use crate::text::{DirtyRect, PlaneUpload};

/// Static identity of an atlas plane texture: its square dimension, texel
/// format, and debug label. Bundled so the upload entry point stays under the
/// argument-count lint.
pub(super) struct PlaneDesc<'a> {
    pub size: u32,
    pub format: TextureFormat,
    pub label: &'a str,
}

fn bytes_per_pixel(format: TextureFormat) -> u32 {
    match format {
        TextureFormat::R8Unorm => 1,
        TextureFormat::Rgba8Unorm => 4,
        _ => panic!("unsupported atlas format: {format:?}"),
    }
}

/// Upload an atlas plane's pending changes to its GPU texture, (re-)creating
/// the texture if the size changed. Returns `true` if a new texture was
/// allocated (so the caller can rebuild the bind group).
///
/// A freshly (re-)allocated texture has no prior contents, so it always takes
/// the full-plane path regardless of `upload`. Otherwise the work follows
/// `upload`: skip entirely on [`PlaneUpload::None`], push only the changed
/// glyph rects on [`PlaneUpload::Rects`], or re-push the whole plane on
/// [`PlaneUpload::Full`].
pub(super) fn write_atlas_plane(
    device: &Device,
    queue: &Queue,
    slot: &mut Option<Texture>,
    data: &[u8],
    desc: PlaneDesc<'_>,
    upload: PlaneUpload<'_>,
) -> bool {
    let PlaneDesc {
        size,
        format,
        label,
    } = desc;
    let needs_new = slot
        .as_ref()
        .is_none_or(|t| t.width() != size || t.height() != size);
    if needs_new {
        *slot = Some(device.create_texture(&TextureDescriptor {
            label: Some(label),
            size: Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        }));
    }
    let texture = slot.as_ref().unwrap();
    let bpp = bytes_per_pixel(format);

    if needs_new || matches!(upload, PlaneUpload::Full) {
        write_region(
            queue,
            texture,
            data,
            size,
            bpp,
            DirtyRect {
                x: 0,
                y: 0,
                w: size,
                h: size,
            },
        );
    } else if let PlaneUpload::Rects(rects) = upload {
        for &rect in rects {
            write_region(queue, texture, data, size, bpp, rect);
        }
    }
    needs_new
}

/// Push one texel rectangle out of the row-major plane `data` into `texture`.
/// The source window is addressed in place: `offset` skips to the rect's first
/// row and `bytes_per_row` is the full plane stride, so each copied row lands
/// at the rect's column in the next plane row without repacking.
fn write_region(
    queue: &Queue,
    texture: &Texture,
    data: &[u8],
    size: u32,
    bpp: u32,
    rect: DirtyRect,
) {
    let stride = size * bpp;
    let offset = u64::from(rect.y * stride + rect.x * bpp);
    queue.write_texture(
        TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: Origin3d {
                x: rect.x,
                y: rect.y,
                z: 0,
            },
            aspect: TextureAspect::All,
        },
        data,
        TexelCopyBufferLayout {
            offset,
            bytes_per_row: Some(stride),
            rows_per_image: None,
        },
        Extent3d {
            width: rect.w,
            height: rect.h,
            depth_or_array_layers: 1,
        },
    );
}

/// View for the given atlas texture, or a 1×1 placeholder when absent.
pub(super) fn atlas_view(
    device: &Device,
    tex: Option<&Texture>,
    format: TextureFormat,
) -> TextureView {
    match tex {
        Some(t) => t.create_view(&TextureViewDescriptor::default()),
        None => device
            .create_texture(&TextureDescriptor {
                label: Some("atlas_placeholder"),
                size: Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format,
                usage: TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&TextureViewDescriptor::default()),
    }
}
