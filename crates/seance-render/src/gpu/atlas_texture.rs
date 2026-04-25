//! Atlas plane upload + placeholder-view helpers. Shared by the grayscale
//! and color glyph atlases; the plane-texture slot is owned by `GpuState`.

use wgpu::*;

fn bytes_per_pixel(format: TextureFormat) -> u32 {
    match format {
        TextureFormat::R8Unorm => 1,
        TextureFormat::Rgba8Unorm => 4,
        _ => panic!("unsupported atlas format: {format:?}"),
    }
}

/// Write `data` into an atlas plane, (re-)creating the texture if the
/// size changed. Returns `true` if a new texture was allocated.
pub(super) fn write_atlas_plane(
    device: &Device,
    queue: &Queue,
    slot: &mut Option<Texture>,
    data: &[u8],
    size: u32,
    format: TextureFormat,
    label: &str,
) -> bool {
    let extent = Extent3d {
        width: size,
        height: size,
        depth_or_array_layers: 1,
    };
    let needs_new = slot
        .as_ref()
        .is_none_or(|t| t.width() != size || t.height() != size);
    if needs_new {
        *slot = Some(device.create_texture(&TextureDescriptor {
            label: Some(label),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        }));
    }
    queue.write_texture(
        TexelCopyTextureInfo {
            texture: slot.as_ref().unwrap(),
            mip_level: 0,
            origin: Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        data,
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size * bytes_per_pixel(format)),
            rows_per_image: None,
        },
        extent,
    );
    needs_new
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
