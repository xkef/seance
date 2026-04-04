# séance

A GPU-rendered terminal aiming at built-in multiplexing — tabs, splits, and
persistent sessions, **without running a terminal inside a terminal**.
macOS-first; Linux is a target.

> **Status:** early. The single-pane terminal renders and runs a shell today.
> The native multiplexer that motivates the name is planned, not built. See
> [Status](#status) below for the honest breakdown.

## The idea

`tmux` and `screen` exist because terminals refused to grow up: if you want
splits or detachable sessions, you nest a second VT parser inside the first.
That nesting is where the pain lives — broken keybindings, double-rendered
glyphs, no native image protocol, no clean GPU path for the inner panes.

séance is meant to be the inverse: one terminal, one VT engine, one GPU
pipeline, with the multiplexer living natively above the grid. Each pane will be
a PTY plugged into its own `libghostty-vt` instance; all panes render into a
single framebuffer with per-pane offsets. No second parser. No second renderer.

The two pieces of the seam:

- **VT state** — `libghostty-vt`, Rust bindings to Ghostty's terminal core.
  CSI/OSC/DCS, alt screen, scrollback, mouse modes, Kitty keyboard, Kitty
  graphics — all handled.
- **GPU** — `wgpu` over Metal/Vulkan/DX12, with a font pipeline built on
  `cosmic-text` (shaping), `swash` (rasterization), and `etagere` (atlas
  packing).

No hand-rolled VT parser. No bespoke graphics abstraction.

## Status

### What works today

- Single-pane terminal: PTY → `libghostty-vt` → wgpu render pass → present.
- Font pipeline: cosmic-text shaping, SwashCache rasterization, two-plane glyph
  atlas (R8 + RGBA8) packed with etagere.
- One render pass, three pipelines: background fill, per-cell background SSBO,
  instanced glyph quads.
- winit key/mouse encoding through `libghostty-vt`'s key encoder; SGR 1006 mouse
  reporting.
- Kitty graphics protocol transmission (recently added).
- 250 Hz PTY poll, redraw only when content is dirty, AutoVsync presentation.
- macOS IOSurface + `presentsWithTransaction` for clean live-resize.

### What's planned

Tracked as GitHub epics M1–M7:

| Epic   | Theme                          | Highlights                                                                                  |
| ------ | ------------------------------ | ------------------------------------------------------------------------------------------- |
| **M1** | Config & themes                | TOML config, hot-reload, theme files, user keybind table                                    |
| **M2** | Rendering performance          | Shape cache, row-dirty flags, DEC 2026 sync output, deadline-driven redraw, atlas batching  |
| **M3** | Visual fidelity                | Procedural box-drawing/Powerline glyphs, WCAG min-contrast, OSC 52 clipboard                |
| **M4** | Z-layer architecture           | `RenderLayer` enum, per-layer vertex buffers, offscreen front/back textures for post-passes |
| **M5** | Image protocols                | Kitty placements, virtual placeholders (U+10EEEE), animated frames                          |
| **M6** | **Multiplexing** (the big one) | `seance-mux` crate, tabs, splits, floating modals, IME preedit                              |
| **M7** | Custom shaders                 | Shadertoy-compatible post-pass with ping-pong textures                                      |

The `seance-mux` crate (M6) is the one that delivers on the project's name. It
will be a `Domain → Window → Tab → SplitTree → Pane` tree that walks itself into
a `Vec<PositionedPane>` each frame; the renderer offsets `grid_pos` by each
pane's top-left and emits all panes' cells into the same render pass. Splits as
1px quads; inactive-pane dimming as a shader uniform.

`docs/architecture.md` tags every subsystem `[IMPLEMENTED]` or `[PLANNED: M<n>]`
— start there for the full picture.

## Pipeline (current)

```
winit ──▶ seance-input ──▶ PTY ──▶ shell
                                    │
                                    ▼
shell ──▶ PTY ──▶ libghostty-vt ──▶ grid state
                                    │
                                    ▼
                          seance-render (wgpu)
                                    │
                          ┌─────────┴─────────┐
                          ▼                   ▼
                     bg cells SSBO       glyph atlas
                          └─────────┬─────────┘
                                    ▼
                         single render pass
                                    │
                                    ▼
                                 present
```

## Crates

| Crate           | Role                                                 | Status       |
| --------------- | ---------------------------------------------------- | ------------ |
| `seance-app`    | winit event loop, top-level `App`, redraw driver     | implemented  |
| `seance-vt`     | `libghostty-vt` adapter, PTY, Kitty graphics         | implemented  |
| `seance-render` | wgpu pipelines, glyph atlas, font shaping            | implemented  |
| `seance-input`  | winit → VT key/mouse encoding, Cmd shortcut dispatch | implemented  |
| `seance-mux`    | Domain / Window / Tab / SplitTree / Pane             | planned (M6) |

Full design reference: [`docs/architecture.md`](docs/architecture.md).

## Build & run

```sh
tools/setup-sysroot.sh       # macOS 26 SDK overlay for Zig's arm64 linker
tools/setup-ghostty-src.sh   # clone + patch vendored ghostty-src
tools/run.sh                 # build and launch
```

## License

Dual-licensed under either of:

- [MIT license](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.
