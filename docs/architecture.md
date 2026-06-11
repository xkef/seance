# Architecture

This document describes both **what is built** and **what is planned**. Each
section is tagged `[IMPLEMENTED]` (present in `main`/feature branches) or
`[PLANNED: M<n>]` (scheduled under a GitHub epic, linked inline).

Epic index:

- **[M1][m1]** — Config & theme foundations
- **[M2][m2]** — Rendering performance (shape cache, dirty rows, sync output,
  deadline redraw, IO thread)
- **[M3][m3]** — Visual fidelity (procedural glyphs, WCAG contrast, clipboard)
- **[M4][m4]** — Z-layer architecture refactor
- **[M5][m5]** — Image protocols (Kitty graphics residuals, animation, iTerm2)
- **[M6][m6]** — Multiplexing (`seance-mux-client` crate, tabs, splits, floating
  modals)
- **[M7][m7]** — Custom shaders (Shadertoy-compatible post-pass)
- **[M8][m8]** — Lua scripting + widget system
- **[M9][m9]** — Release pipeline & distribution (Homebrew, AUR, apt)
- **[M10][m10]** — Agent Plane (in-PTY control, UI ownership, coordination)
- **[M11][m11]** — Test harness (layered, LLM-readable)
- **[M12][m12]** — Client/server multi-domain (Local / Unix / SSH / TLS)

[m1]: https://github.com/xkef/seance/issues/4
[m2]: https://github.com/xkef/seance/issues/5
[m3]: https://github.com/xkef/seance/issues/6
[m4]: https://github.com/xkef/seance/issues/7
[m5]: https://github.com/xkef/seance/issues/8
[m6]: https://github.com/xkef/seance/issues/9
[m7]: https://github.com/xkef/seance/issues/10
[m8]: https://github.com/xkef/seance/issues/65
[m9]: https://github.com/xkef/seance/issues/152
[m10]: https://github.com/xkef/seance/issues/194
[m11]: https://github.com/xkef/seance/issues/201
[m12]: https://github.com/xkef/seance/issues/221

---

## Pipeline overview

```
┌─ input ──────────────────────────────────────────────────────────────┐
│ winit event loop → seance-input                                      │
│   key: KeyboardEvent → libghostty-vt key encoder → bytes             │
│   mouse: click/drag → SGR 1006; wheel → 4/5, alt-arrows, or scroll   │
│ UI sends mux PaneInput/ResizePane/ScrollLines                        │
│ Local mux forwards to VT Actor, which writes PTY ─────────▶ shell    │
└──────────────────────────────────────────────────────────────────────┘

┌─ VT/PTY actor ───────────────────────────────────────────────────────┐
│ Unix VT Actor owns PTY + VT Core                                     │
│   VT Core owns libghostty Terminal/RenderState + Kitty setup         │
│   nonblocking poll → read PTY → VT Core vt_write()                   │
│   DEC 2026 gate controls snapshot publication                        │
│   publish owned Arc<VtSnapshot> → LocalDomain → MuxClient/PaneView   │
│ UI renders PaneView/FrameSource; it never reads live libghostty state│
│ UI acks presented VT Snapshot generations after successful present   │
└──────────────────────────────────────────────────────────────────────┘

┌─ render pass (wakes on dirty + animation deadline) ──────────────────┐
│ rebuild_cells(only_dirty_rows [PARTIAL: bg_cells]):                  │
│   for each row: run-iterator → TextRuns                              │
│     shape_cache.get_or_shape(run_hash)  [IMPLEMENTED]                │
│       cosmic-text Buffer::shape_until_scroll                         │
│     for each glyph:                                                  │
│       procedural sprite registry [PLANNED: M3]                       │
│       glyph_cache.get_or_insert(CacheKey)                            │
│         miss → SwashCache → bitmap → etagere atlas                   │
│                                                                      │
│ i32-keyed layer schedule -> per-op draws [IMPLEMENTED]               │
│ single render pass → N pipeline switches                             │
│ optional post-pass (custom shaders, ping-pong) [PLANNED: M7]         │
│ present()                                                            │
└──────────────────────────────────────────────────────────────────────┘
```

---

## Crate structure

| Crate                | Owns                                                                                                                            | Status               |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------- | -------------------- |
| `seance-app`         | winit event loop, `App`, renderer/redraw driver, in-process server bootstrap                                                    | [IMPLEMENTED]        |
| `seance-protocol`    | wire protocol, owned frame data, transport codec, identities, clipboard data                                                    | [IMPLEMENTED/M6+M12] |
| `seance-frame`       | render-facing `FrameSource` trait and borrowed visitor types                                                                    | [IMPLEMENTED/M6]     |
| `seance-input`       | winit → VT key/mouse encoding (via libghostty-vt)                                                                               | [IMPLEMENTED]        |
| `seance-render`      | font pipeline, GPU pipelines, GlyphAtlas, image cache                                                                           | [IMPLEMENTED]        |
| `seance-vt`          | VT Core, PTY actor, snapshot/command API                                                                                        | [IMPLEMENTED/M2]     |
| `seance-config`      | TOML config + theme files, hot-reload, diffing                                                                                  | [IMPLEMENTED]        |
| `seance-mux-client`  | Client-side: `Domain` trait, `MuxClient`/`PaneView`, `ProtocolDomain`. No VT dependency.                                        | [IMPLEMENTED/M6+M12] |
| `seance-mux-server`  | Server-side: `LocalDomain` (owns VTs via `seance-vt`) + `serve()` protocol dispatch + `spawn_local_server` in-process bootstrap | [IMPLEMENTED/M12]    |
| `seance-bench`       | frame-time benches for renderer hot paths                                                                                       | [IMPLEMENTED]        |
| `seance-render-test` | render-harness fixtures (L1 logic, L4 frame snapshot)                                                                           | [IMPLEMENTED]        |

Current dependency direction:

```text
seance-app         -> seance-mux-client, seance-mux-server, seance-render,
                      seance-input, seance-config
seance-mux-client         -> seance-protocol, seance-frame
seance-mux-server  -> seance-mux-client, seance-protocol, seance-vt
seance-vt          -> seance-protocol, seance-frame
seance-render      -> seance-protocol, seance-frame, seance-config
seance-input       -> seance-protocol
seance-render-test -> seance-mux-client, seance-protocol, seance-frame, seance-vt
```

`seance-mux-client` deliberately omits any `seance-vt` edge: a protocol-only
client (in-process today, a remote thin client under [M12][m12] tomorrow) builds
without libghostty linked. The local-mode binary pairs `seance-mux-client`
(frontend) with `seance-mux-server` (backend) over an `InProcessTransport`.

---

## VT layer (`seance-vt`)

- **libghostty-vt** [IMPLEMENTED] — VT state machine via FFI. Handles
  CSI/OSC/DCS, alt screen, scrollback, mouse modes, Kitty keyboard.
- **portable-pty** [IMPLEMENTED] — production PTY Adapter; M2 actor v1 uses Unix
  raw-fd readiness polling. Actor tests use a private scripted Adapter.
- **VT Core** [IMPLEMENTED] — owns live libghostty `Terminal`, persistent
  `RenderState`, Kitty setup, cursor/theme seeding, snapshot extraction, and
  dirty-row generation tracking. VT Actor and Headless VT both wrap this Module.
- **FrameSource** trait [IMPLEMENTED] — lives in `seance-frame` and exposes
  `visit_cells()` to the renderer without depending on `seance-vt`.
- **Owned snapshots** [IMPLEMENTED] — `VtSnapshot` lives in `seance-protocol`,
  is built by VT Core, and is read through `SnapshotFrameSource`; live
  libghostty state is never shared with the UI and there is no public
  live-terminal `FrameSource` adapter.
- **Row-dirty flags** [IMPLEMENTED] — `VtSnapshot::dirty` reports rows changed
  since the last successfully rendered generation acknowledged by the Pane
  Session. The renderer uses it for partial `bg_cells` upload (#196); text-cell
  rebuild still walks the full grid pending shape cache (#21).
- **DEC 2026 synchronized output** [IMPLEMENTED] — VT Actor publication gate
  with a 150 ms watchdog.
- **OSC 52 clipboard** [IMPLEMENTED] — VT Core parses
  `OSC 52 ; <sel> ; <base64|?> ST`, emits `ClipboardRequest::{Write, Read}`
  through the VT actor, and the App layer
  (`SurfaceState::handle_clipboard_request`) routes them through `arboard`,
  echoing reads back via an OSC 52 reply. Gated by
  `clipboard.{read,write} = "allow" | "ask" | "deny"`, both default `deny`;
  users opt in by setting `allow` (or `ask` once the M3 confirm-overlay UI
  ships).
- **Kitty graphics protocol** [IMPLEMENTED] — transmission, decode (PNG and raw
  24/32-bit), per-image cache with 320 MB storage cap, placement resolution.
  Virtual placeholders (U+10EEEE), animation, and iTerm2 inline images remain
  [PLANNED: [M5][m5]].

---

## Renderer (`seance-render`)

### Font pipeline

- **cosmic-text** [IMPLEMENTED] — wraps fontdb + rustybuzz + bidi. Shapes
  contiguous same-style runs of cells through `TextBackend::shape_run`, so
  ligatures (`==`, `=>`), regional flag pairs, and ZWJ sequences compose across
  cell boundaries; each emitted glyph carries its source-cluster byte offset so
  the cell builder anchors it at the originating column.
- **OpenType features** [IMPLEMENTED] — `font.features` is parsed into a
  cosmic-text `FontFeatures` list and applied via `Attrs::font_features`.
- **SwashCache** [IMPLEMENTED] — rasterizes outlines (COLR v0/v1, SVG, CBDT).
- **GlyphAtlas** [IMPLEMENTED] — two planes: grayscale R8 (2048×2048) and color
  RGBA8 (1024×1024). `etagere` rectangle packing, per-plane `dirty` flag.
- **GlyphCache** [IMPLEMENTED] — `FxHashMap<cosmic_text::CacheKey, AtlasEntry>`.
- **Shape cache** keyed by `(font flags, run bytes)` [IMPLEMENTED] — 256-bucket
  × 8-way LRU; the key omits color so per-frame palette flicker doesn't evict.
- **Procedural sprite registry** (codepoints > U+10FFFF, U+2500–U+259F,
  U+E0B0–U+E0B3, legacy computing, braille) [PLANNED: [M3][m3]] — rasterized via
  `tiny-skia`, intercepted before cosmic-text shaping.

### CellBuilder

- **Current** [IMPLEMENTED] — iterates entire VT grid each frame, groups
  contiguous same-style cells into shape runs, dispatches each run through
  `TextBackend::shape_run`, then anchors each emitted glyph at its
  source-cluster column before writing `text_cells` SSBO data; `bg_cells` upload
  is dirty-row-batched (#196).
- **Target** — takes `&[PositionedPane]` [PLANNED: [M6][m6]], only iterates
  dirty rows for text rebuild [PLANNED: [M2][m2]].

### CellText instance layout (matches WGSL, 32 bytes)

| Offset | Field             | Type    | Purpose                                                                            |
| ------ | ----------------- | ------- | ---------------------------------------------------------------------------------- |
| 0      | `glyph_pos`       | `u32×2` | atlas pixel coords                                                                 |
| 8      | `glyph_size`      | `u32×2` | bitmap dimensions                                                                  |
| 16     | `bearings`        | `i16×2` | x/y bearing                                                                        |
| 20     | `grid_pos`        | `u16×2` | column, row                                                                        |
| 24     | `color`           | `u8×4`  | RGBA foreground (Unorm8x4)                                                         |
| 28     | `atlas_and_flags` | `u32`   | low byte: atlas (0=gray,1=color); byte 1: cursor flags; byte 2 bit 0: min-contrast |

---

## GPU layers

### Layer schedule [IMPLEMENTED]

One `wgpu::RenderPass` drives a dynamic `i32`-keyed layer schedule
(`crates/seance-render/src/gpu/layers.rs`), sorted CPU-side, no depth buffer —
not a closed enum. Layers are created on demand (`layer_for_z(z)`) and kept
sorted by z-index; the renderer enumerates no product features. Two axes:

- **Layer = open `i32` z.** Well-known positions are `const` (`Z_MAIN = 0`, a
  `Z_WINDOW_BG` band below it); new overlays (status/tab bar, command palette,
  IME preedit, split borders) pick their own z and the renderer stays agnostic.
  Mirrors WezTerm's `layer_for_zindex` (a sorted `Vec` created on demand),
  adapted to seance's heterogeneous draw ops.
- **Sub-role = fixed within-layer order**, `Below → Content → Above`.

A layer holds `DrawOp` tags, not GPU state; the frame's render pass walks the
schedule and binds the matching pipeline per op. The four op kinds share 3 bind
groups (uniforms / bg_cells SSBO / atlas textures + sampler):

| Op             | Vertex              | Fragment                                      | Blend               |
| -------------- | ------------------- | --------------------------------------------- | ------------------- |
| `BgColorFill`  | fullscreen triangle | solid uniforms.bg_color                       | none                |
| `CellBg`       | fullscreen triangle | per-cell bg from SSBO + cursor shapes         | premultiplied alpha |
| `CellText`     | instanced quads     | atlas sample, min-contrast, cursor color swap | premultiplied alpha |
| `Images(band)` | instanced quads     | per-image texture sample                      | premultiplied alpha |

The terminal cell content is the reference plane, not a single z, so the three
Kitty `PlacementLayer` bands live as sub-roles of the `Z_MAIN` layer rather than
as separate layers; a Kitty image at z=-1 thus draws after `cell_bg` and before
text. Within a band, placements keep their raw-`i32`-z sort. Draw order at
`Z_MAIN`:

```
SubRole::Below     bg_color fill → Kitty below-bg → cell_bg SSBO → Kitty below-text
SubRole::Content   cell_text (glyphs + sprite underlines + cursor glyph)
SubRole::Above     Kitty above-text
```

Stacked window backgrounds sit at negative z; floating UI at positive z — each a
`layer_for_z(z)` call, no type edits. Selection is resolved on the CPU into the
`bg_cells` SSBO and per-glyph colors (Ghostty-style); cursor-over-text stays
baked into the `cell_bg`/`cell_text` shaders, and promoting it to a distinct
`Above` op is future work.

### Offscreen post-pass infrastructure [PLANNED: [M4][m4] + [M7][m7]]

Front/back `bgra8unorm_srgb` render textures sized to the surface. All layers
target `back`; optional ping-pong of user-supplied Shadertoy-compatible shaders;
final blit to the drawable. The offscreen front/back pair + blit is the
remaining M4 exit criterion.

### Atlas upload

`wgpu::Queue::write_texture` per inserted glyph. Migrate to dirty-sub-rect
batching [PLANNED: [M2][m2]].

---

## Event loop & redraw

### Current [IMPLEMENTED]

Deadline-scheduled (`cf4a1b1`, #24): `ControlFlow::WaitUntil(next_due)` across
all animation sources — cursor blink, SGR blink, bell, Kitty GIF frames,
custom-shader animation. Idle terminal = 0 fps. Modelled on WezTerm's
`has_animation` pattern. PTY wakes are out-of-band via `EventLoopProxy`, fed by
pane-scoped `MuxEvent` values from `seance-mux-client`; the UI then asks the Mux
Client to drain ordered Domain events into Pane Views.

### Threading model

VT parsing and all libghostty state live inside VT Core on a Unix VT Actor. The
actor owns PTY reads/writes and publishes owned `Arc<VtSnapshot>` values through
its local session API. `LocalDomain` wraps that API, stamps pane identity onto
local events, and publishes ordered Pane Updates. `MuxClient` applies those
updates into Pane Views and exposes app-facing Pane Handles. The UI renders a
Pane View via `seance-frame` and acknowledges the presented VT Snapshot
generation after a successful present.

Resize follows the same rule: the UI computes the new grid size, sends a resize
command, and redraws after the actor publishes the resized snapshot rather than
forcing an immediate stale-frame draw.

Full design, actor API, snapshot model, DEC 2026 watchdog, shutdown ordering,
and the renderer-thread revisit metric: see
[`docs/threading.md`](./threading.md).

---

## Multiplexing model [IMPLEMENTED/M6+M12 + PLANNED: [M6][m6], [M12][m12]]

The frontend (renderer, input, window — everything in `seance-app` plus
`seance-render` and `seance-input`) communicates with the VT-owning backend only
through the wire protocol in `seance-protocol`. In local mode that protocol runs
over an `InProcessTransport` between two threads of the same process; remote
modes ([M12][m12]) swap the transport, not the contract.

- Frontend: `MuxClient<ProtocolDomain<Transport>>`. Sends `ClientMessage`s,
  applies `ServerMessage`s into per-pane `PaneView`s, exposes `PaneHandle`
  operations to the app layer.
- Backend: `LocalDomain` owns the VT Actor and per-pane replay history;
  `seance-mux-server::serve` drains `ClientMessage`s, dispatches each to the
  Domain, and pushes `DomainEvent`s back as `ServerMessage`s.
- Bootstrap: `seance-mux-server::spawn_local_server` pairs a fresh `LocalDomain`
  with an `InProcessTransport`, runs `serve` on a background thread, and returns
  the client end the frontend hands to `ProtocolDomain`. The `wake` closure
  passed in is what fires `EventLoopProxy::send_event(UserEvent::Mux(Wake))`
  whenever the server emits a frame.

Full #45 mux topology remains planned (multi-pane / tab / split UX):

```
Domain (trait)                       ← LocalDomain or any remote Domain
  └─ Window
       └─ Tab
            └─ SplitTree = Leaf(Pane) | Split(dir, ratio, left, right)
```

`fn panes_positioned(&self, pixel_rect: Rect) -> Vec<PositionedPane>` walks the
tree and emits per-pane `{ cell_rect, pixel_rect, is_active, pane }`.
`CellBuilder` offsets `grid_pos` by each pane's top-left. **All panes render
into one framebuffer** — no render-target-per-pane.

- Split borders: 1px quads via floating-quad emitter (`RenderLayer::Selection`).
- Inactive-pane dimming: shader uniform `inactive_pane_hsb: vec3<f32>`, applied
  to fg when `pane_idx != active_pane_idx`.
- Tab bar: reserved row rendered through the status-line path.
- Floating modals (palette, char select): `taffy` box-model →
  `RenderLayer::Modal`.
- IME preedit: winit `Ime::Preedit` → shape inline at cursor column,
  `RenderLayer::ImePreedit`.

### Domain seam ([M12][m12])

Constraints carried forward from #221 so phases 2–5 don't redesign:

- No process-local handles (`&File`, `&Pty`) in trait return types.
  [IMPLEMENTED]
- `PaneRef` carries a `DomainId` so two `Domain` instances mint non-overlapping
  IDs. [IMPLEMENTED]
- Renderer-facing access is `PaneView::frame_source()`, not `Pane::vt()`. Remote
  panes (where the VT lives on the other end) satisfy the same `FrameSource`
  interface as local panes. [IMPLEMENTED]
- `Domain::spawn_pane` may evolve to a future or callback form once a non-local
  transport that can't be synchronously round-tripped lands. Today it is
  synchronous, blocking on the `ClientMessage::SpawnPane` →
  `ServerMessage::Topology` round-trip via `ProtocolDomain`'s in-flight request
  table. [PLANNED: [M12][m12] Phase 2]

Phase 2 (`UnixDomain`) becomes "add a Unix-socket `Transport` impl and a
`seance-mux-server` daemon binary"; no client refactor is required. The protocol
shape is canonicalized in [`docs/protocol.md`](./protocol.md).

---

## Config surface

### Current [IMPLEMENTED]

Compile-time defaults only. `RendererConfig` exposes `width`, `height`, `scale`,
`font_family` (hardcoded "JetBrainsMono Nerd Font"), `font_size`
(runtime-adjustable via keybinds). Theme is `impl Default` → Catppuccin Frappe
palette.

### Target [PLANNED: [M1][m1]]

`~/.config/seance/config.toml` (or `$XDG_CONFIG_HOME/seance/config.toml`) with
sections `[font]`, `[window]`, `[cursor]`, `[clipboard]`, `[scrollback]`,
`[input]`, `[[keybind]]`, `[renderer]`, plus a top-level `theme = "<name>"` that
resolves against `~/.config/seance/themes/`.

Canonical schema:
[`seance_config::Config`](../crates/seance-config/src/schema.rs) and its
sub-structs (`FontConfig`, `WindowConfig`, `ClipboardConfig`, …). Theme files
ship Catppuccin / Gruvbox / Tokyo Night / Solarized.

Hot-reload via `notify` classifies each change through
[`ConfigDiff`](../crates/seance-config/src/diff.rs) and targets the right
invalidation: theme → repaint; font → clear glyph + shape caches; keybind →
rebuild action table.

---

## Input

- winit `KeyboardInput` → `seance-input` → libghostty-vt key encoder
  [IMPLEMENTED].
- `input.macos_option_as_alt` = `none` / `left` / `right` / `both`
  [IMPLEMENTED]. When `left` or `right`, only that side of Option sends
  `ESC`-prefix; the other side falls through to macOS text composition (`ø`,
  `¬`, `–`, …). `both` makes both Option keys Alt (breaks text composition);
  `none` (default) preserves the macOS default.
- User keybind table parsed from config `[[keybind]]` entries → `Action` enum
  [IMPLEMENTED: [M1][m1]]. Built-in defaults (the Cmd shortcuts) seed the table;
  a user entry overrides by chord and an `unbind` action removes one. Dispatch
  actions wired today (`Copy`, `Paste`, `SelectAll`, `Quit`, `CloseSurface`,
  `FontSize(i8)`, `ResetFontSize`, `ToggleFullscreen`) run; mux actions
  (`NewTab`, `SplitH`, `FocusPane(Dir)`, `SwitchTab`, `Scroll(Dir)`, …) parse
  but are logged as not-yet-implemented pending [M6][m6].

---

## Platform notes

- macOS IOSurface / `CAMetalLayer.presentsWithTransaction = true` to prevent
  live-resize stretching [IMPLEMENTED].
- macOS 26.4 SDK + Zig 0.15 linker workaround (`tools/xcrun` redirects SDK
  sysroot to Zig's bundled `libSystem.tbd`) [IMPLEMENTED]. Retire by bumping
  `.mise.toml`'s zig pin to `0.16.x`, which contains the upstream fix
  (`ziglang/zig#31673` on Codeberg).
- Wayland damage tracking via `swap_buffers_with_damage` is not required for
  wgpu — dirty-row uploads + deadline-driven redraw replace it.

---

## Appendix — component choices

| Problem                   | Component                                             | Why                                                                                                                                                                 |
| ------------------------- | ----------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| GPU API                   | `wgpu`                                                | One abstraction for Metal/Vulkan/DX12/GL4/WebGPU. Dual-source blending (for LCD subpixel AA) gated behind `Features::DUAL_SOURCE_BLENDING`.                         |
| Window + input            | `winit`                                               | Only serious cross-platform option.                                                                                                                                 |
| VT state machine          | `libghostty-vt` via FFI                               | Battle-tested, handles DEC 2026, mouse, Kitty keyboard, iTerm OSC, selection. Don't reinvent.                                                                       |
| PTY                       | `portable-pty`                                        | Cross-plat, correct ConPTY on Windows.                                                                                                                              |
| Font discovery            | `fontdb` (via cosmic-text)                            | fontconfig / CoreText / DirectWrite backed.                                                                                                                         |
| Shaping                   | `cosmic-text` (rustybuzz + unicode-bidi)              | BiDi, graphemes, per-font features.                                                                                                                                 |
| Rasterization             | `swash` (via `SwashCache`)                            | COLR v0/v1, SVG, CBDT.                                                                                                                                              |
| Atlas packing             | `etagere`                                             | Shelf-bin with deallocation (alacritty's row-packer cannot evict).                                                                                                  |
| Procedural glyphs         | `tiny-skia`                                           | Software vector rasterizer; box-drawing (U+2500–U+257F) and block elements (U+2580–U+259F) shipped, Powerline sprites pending [partial: [M3][m3]].                  |
| Layout (modals/box model) | `taffy`                                               | Flexbox + Grid for floating UI [PLANNED: [M6][m6]].                                                                                                                 |
| Animation                 | Deadline scheduler [IMPLEMENTED]                      | `ControlFlow::WaitUntil(min(next_due))` across cursor blink / SGR blink / bell / Kitty animation — idle terminal draws nothing.                                     |
| Config                    | `toml` + `serde` + `notify`                           | Hot-reload with targeted invalidation.                                                                                                                              |
| Logging                   | `tracing` + `tracing-subscriber` + `tracing-appender` | Structured events + spans; `EnvFilter` honors `RUST_LOG`; non-blocking daily-rolling file at `~/Library/Logs/seance/` (macOS) or `$XDG_STATE_HOME/seance/` (Linux). |

**Deliberately avoided:** `fontdue` (no COLRv1/SVG), `glyphon` (locks layout),
`vello`/`wgpu_glyph` (wrong abstraction level for terminals), hand-rolled VT
parsers (tarpit — every terminal team regrets them).

---

## Reference terminals

For threading-model rationale (Ghostty / Alacritty / WezTerm side-by-side), see
[`docs/threading.md`](./threading.md). Source citations into each upstream tree
live there, not here.

---

## Logging & instrumentation

`tracing` facade; `EnvFilter` honors `RUST_LOG`. Stdout layer plus a
non-blocking daily-rolling file at `~/Library/Logs/seance/` (macOS) or
`$XDG_STATE_HOME/seance/` with `~/.local/state/seance/` fallback (Linux).
Subscriber setup and the default filter live in
[`crates/seance-app/src/main.rs`](../crates/seance-app/src/main.rs) — that file
is the source of truth for span placement and level conventions.
