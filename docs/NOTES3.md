  Your lib_renderer_c.zig imports the same Zig modules that libghostty-vt wraps (terminal/main.zig, terminal/stream_terminal.zig). So the underlying VT parsing, terminal state, reflow, scrollback —
  all the same code. But you access it via Zig @import, not via the exported ghostty_terminal_* C functions from lib_vt.zig.

  On the seance side, your Rust code then calls your own C API (ghostty_terminal_new etc. from lib_renderer_c.zig), which wraps the Zig terminal directly.

  What would make this upstreamable

  1. Decouple the terminal from the renderer API. Have libghostty-renderer accept a GhosttyTerminal from libghostty-vt rather than creating its own. This means the renderer header should take a
  handle from the vt library, not define its own terminal lifecycle. Upstream will want the two libraries independent.
  2. Use the upstream RenderState (include/ghostty/vt/render.h) as the bridge between vt and renderer, rather than having the renderer reach directly into terminal.Terminal internals.
  3. Fill in the stubs — theme loading is especially important since it's a selling point of the Ghostty ecosystem.
  4. Split the header: renderer.h for GPU rendering, rely on vt.h for terminal. Consumers include both.
  5. The Options.zig changes are the most upstreamable part — making those fields optional is a clean, minimal change that enables standalone use without breaking existing apprt consumers. That
  could be a focused PR on its own.
