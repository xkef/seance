# mmterm -- Build Plan

## Vision

A terminal emulator built on libghostty that provides all tmux features
without the terminal-in-a-terminal architecture. Single-surface GPU
rendering for flicker-free split management.

## Tech Stack

- **Language:** Rust
- **Terminal emulation:** libghostty-vt (via libghostty-rs)
- **GPU rendering:** wgpu (Metal on macOS, Vulkan on Linux)
- **Windowing:** winit
- **Font shaping:** rustybuzz (Rust HarfBuzz port)
- **VCS:** jj (Jujutsu)
- **Toolchain management:** mise (Rust + Zig)

## Architecture

- Single GPU surface per window -- all panes, dividers, overlays, status
  bars rendered in one pass. No platform widget resize race.
- One libghostty-vt instance per pane -- parses PTY bytes into structured
  cell/attribute data. No escape code re-encoding.
- Scene graph with batched instanced draw calls (inspired by Zed's GPUI).
- Binary tree pane layout (inspired by WezTerm).
- Cell-grid-aware layout -- split boundaries snap to cell grid lines.

## Build Sequence

### Phase 1: Single-pane terminal (MVP)
1. **Ghostling clone** -- single libghostty-vt instance + wgpu renderer.
   Get a working terminal with font rendering, input handling, basic
   scrollback.
   - Set up wgpu + winit scaffolding
   - Integrate libghostty-rs for VT emulation
   - Build glyph atlas with rustybuzz shaping
   - Implement cell grid renderer (instanced draw calls)
   - Wire up PTY I/O on a separate thread
   - Handle resize (SIGWINCH + grid reflow + GPU buffer rebuild)

### Phase 2: Single-surface split rendering
2. Two+ libghostty-vt instances, one wgpu surface, manual layout.
   - Binary tree pane model
   - Viewport subdivision and scissor rects
   - Resize without flicker (presentsWithTransaction on macOS)
   - Divider rendering

### Phase 3: Input layer
3. Prefix-key modal system, pane navigation, split create/close/zoom.
   - Key table / modal input state machine
   - Configurable keybindings
   - Smart-splits (detect foreground process for vim/neovim awareness)

### Phase 4: Copy mode
4. Vim motions over scrollback, visual selection, search.
   - Query libghostty-vt scrollback buffer directly
   - v/V/C-v selection modes
   - Incremental search with match count
   - OSC 133 prompt jump
   - Clipboard integration

### Phase 5: Overlays
5. Popup panels and hint-copy as composited layers.
   - Floating pane support (for lazygit, fzf, etc.)
   - Hint-copy overlay (easymotion-style)
   - URL picker from scrollback

### Phase 6: Session/workspace model
6. Project-based sessions with directory association.
   - Workspace serialization/restore
   - Zoxide integration
   - Named sessions

### Phase 7: Mux daemon
7. Headless server for detach/reattach.
   - Extract PTY + libghostty-vt into daemon process
   - Structured protocol over Unix socket (inspired by WezTerm PDU)
   - Dirty line tracking for efficient client updates
   - GUI reconnection

## Key Design Decisions

### Rendering
- wgpu over direct Metal: cross-platform, production-proven (Zed), minimal overhead
- Instanced rendering: one quad per cell, per-instance attributes (position, atlas UV, colors)
- Multi-texture glyph atlas from the start (avoid exhaustion issues)
- Sub-pixel positioning: 1/3-pixel grid with 3 rasterized variants (Warp's approach)
- SDF-based shapes for cursors, borders, rounded rects

### Concurrency
- Lock-free message passing between PTY and render threads (Ghostty's approach)
- updateFrame() under lock, drawFrame() lock-free
- Cache expensive syscalls (tcgetpgrp ~700us) with TTL

### Flicker-free resize
- CAMetalLayer.presentsWithTransaction = true during resize
- commandBuffer.waitUntilScheduled() during resize, async present otherwise
- Triple-buffered instance buffers
- Generation counters for cache invalidation (WezTerm pattern)

## Risks

- libghostty-vt is alpha -- breaking API changes expected
- libghostty-rs is 2 weeks old (as of 2026-04-04), sole maintainer
- Renderer must be built from scratch (GPU rendering not in libghostty)
- Single-surface compositing = no platform accessibility, drag handles, etc.
- Zig 0.15.x build dependency for libghostty-rs

## Prior Art References

- ghostty-org/ghostling -- official minimal libghostty C API example
- Uzaaft/libghostty-rs -- Rust bindings (only option)
- neurosnap/zmx -- session persistence with libghostty-vt
- Zed GPUI -- single-surface rendering, scene graph, instanced draw calls
- WezTerm -- mux server architecture, binary tree pane model, caching hierarchy
- Alacritty -- glyph atlas, instanced cell rendering
- Rio/Sugarloaf -- wgpu-based terminal rendering
- cosmic-term -- alacritty_terminal + wgpu via iced
