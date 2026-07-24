# Theme color reproduction audit vs Ghostty

Audit for #19 (Epic M1 — Config & theme foundations).

Since #12, séance and Ghostty load the exact same theme bytes, so any visual
delta between the two isolates to renderer color math rather than palette drift.
This is a code-level audit of that math: the surface format, the blend equation,
the sRGB/linear handling, the min-contrast correction, and background-opacity
alpha. It reaches the same conclusions a side-by-side capture would, at the
resolution of specific `file:line` references, and files follow-up issues for
the two deltas it found. A visual side-by-side against `kitty.app`/Ghostty
remains a useful complementary check but is not required to close out the
questions #19 poses.

All references below are to `crates/seance-render` unless noted otherwise.

## Summary

| Axis                       | Verdict                                                                        |
| -------------------------- | ------------------------------------------------------------------------------ |
| sRGB vs linear blending    | Faithful — gamma-space blend on a non-sRGB surface, matching Ghostty's default |
| min-contrast correction    | Faithful — WCAG relative luminance in linearized sRGB                          |
| background opacity / alpha | Faithful — one premultiplied-alpha pipeline end to end                         |
| bold-is-bright             | Delta — not implemented; tracked by #175                                       |
| color-glyph (emoji) alpha  | Delta — straight-alpha atlas data fed to a premultiplied blend                 |

## Surface format and the blend equation

`GpuState::new` deliberately selects the first **non-sRGB** surface format the
adapter offers, falling back to `formats[0]` only if every candidate is sRGB
(`src/gpu/state.rs:84-89`). Every pipeline is created with a single
premultiplied blend state — `src_factor = One`, `dst_factor = OneMinusSrcAlpha`,
`operation = Add`, applied identically to the color and alpha channels
(`src/gpu/pipeline.rs:125-132`).

These two choices are a matched pair, and they define the whole color model:

- Because the surface is not sRGB, the GPU performs **no** automatic linear→sRGB
  encode on store and no sRGB→linear decode on blend. Fragment outputs land in
  the framebuffer verbatim and compositing arithmetic happens in gamma (sRGB)
  space.
- Because the blend is premultiplied, every fragment shader must output
  `rgb * a` in the color channels.

This matches Ghostty's default `alpha-blending = native`, which likewise
composites in gamma space. Ghostty's opt-in `linear`/`linear-corrected` modes
have no séance equivalent yet; that is a deliberate scope choice, not a bug, and
is out of scope for #19.

## sRGB vs linear-space blending

There is no CPU-side gamma conversion anywhere in the color path. Colors reach
the GPU as raw sRGB bytes scaled to `[0, 1]`: `u8x4_to_f32` and the theme/bg
packers in `src/gpu/uniforms.rs:104-160` only divide by 255. The cell background
storage buffer packs the same way and `unpack_rgba` in `cell.wgsl` reverses it
without a transfer function.

The only place the shader linearizes is inside min-contrast (see below), which
is correct: WCAG luminance is defined on linearized components, so min-contrast
must linearize even though the surrounding blend stays in gamma space.

Verdict: consistent and faithful. Blending in gamma space on a non-sRGB surface
with no double conversion is exactly the intended model and matches Ghostty's
default.

## min-contrast correction

`apply_min_contrast` (`src/gpu/shaders/cell.wgsl`) implements the WCAG 2.x
contrast rule:

1. `linearize_component` applies the standard sRGB EOTF with the `0.04045`
   threshold, `/ 12.92` toe, and `((v + 0.055) / 1.055) ^ 2.4` curve.
2. `luminance` uses the Rec. 709 coefficients `(0.2126, 0.7152, 0.0722)`.
3. `contrast_ratio` is `(max(L) + 0.05) / (min(L) + 0.05)`.
4. When `fg`/`bg` fall below the configured ratio, the glyph snaps to whichever
   of pure white or pure black yields the higher contrast against `bg`.

This is the same algorithm and the same early-out (`min_ratio <= 1.0` returns
`fg` unchanged) as Ghostty's `contrasted_color`. The correction range is driven
by the `font.min_contrast: f32` config knob (default `1.0`, i.e. disabled;
`crates/seance-config/src/schema.rs`), threaded through as the global
`uniforms.min_contrast` and gated per cell by `TEXT_FLAG_MIN_CONTRAST`
(`src/text/cell_builder.rs:43,420-423`). Only grayscale (mask) glyphs consult
it; the color-glyph branch skips it, which is correct because emoji carry their
own color.

Verdict: faithful, including the white/black snap behavior.

## Background opacity and alpha

Background opacity flows through the same premultiplied contract:

- The window background pass outputs `bg.rgb * bg.a` with `bg.a` preserved
  (`fs_bg_color`), so a translucent `bg_color` writes a premultiplied,
  correctly-attenuated framebuffer value.
- `fs_cell_bg` composites the cursor overlay and hovered-link underline with
  `mix(...)` in gamma space and then premultiplies once at the end
  (`color.rgb * color.a`).
- Selection paints the cell background fully opaque (`sel.rgb, 1.0`) so glyphs
  sit on a solid block rather than an alpha tint — an intentional tmux-style
  choice, documented in the shader.
- The grayscale text branch outputs `fg * (mask_a * color_a)` with matching
  alpha, so faint text (SGR 2, `FAINT_ALPHA` in `cell_builder.rs`) attenuates
  correctly.

Every producer premultiplies exactly once, and the blend consumes premultiplied
input. Verdict: faithful.

## Deltas found

### 1. bold-is-bright is not implemented

Ghostty's `bold-is-bright` remaps SGR 1 on ANSI palette indices 0–7 to their
bright counterparts 8–15. séance has no equivalent: `cell_builder` resolves the
foreground from the base palette without a bold→bright step. This is a known gap
already tracked by #175 (open, with in-flight PRs), so no new issue is filed for
it here. Until it lands, bold text on the 16-color palette will read dimmer than
Ghostty's default.

### 2. Color-glyph (emoji) alpha is not premultiplied for the blend

The color-glyph path is the one place the premultiplied-alpha contract is not
upheld:

- swash returns `SwashContent::Color` images as **straight-alpha** RGBA, and
  `CosmicBackend::rasterize` clones that buffer verbatim
  (`src/text/cosmic.rs:207-220`).
- `GlyphAtlas::insert` copies glyph bytes into the color atlas with a plain
  `copy_from_slice`, applying no premultiplication (`src/text/atlas.rs`).
- The atlas is `Rgba8Unorm` (`src/gpu/state.rs:21`) and the color branch of
  `fs_cell_text` returns the sampled texel directly, unmodified, into the
  premultiplied blend (`cell.wgsl`).

For fully-opaque texels (`a = 1`) straight and premultiplied encodings coincide,
so emoji interiors are correct. Only the antialiased edge texels (`0 < a < 1`)
differ: the blend computes `rgb + dst * (1 - a)` where correctness wants
`rgb * a + dst * (1 - a)`, so edge pixels are over-bright by `rgb * (1 - a)`.
The artifact is a faint light halo around emoji, most visible on light
backgrounds and negligible on black. This is filed as #314 under Epic M3 (visual
fidelity). The likely fix is to premultiply color-glyph RGBA at atlas insert
time, which needs no shader change.

## Follow-ups

- #175 — bold-is-bright (already open; not re-filed).
- #314 — premultiply color-glyph alpha so emoji compositing matches the
  premultiplied blend (filed with this audit).
