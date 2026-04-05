Summary: libghostty-based terminal emulator with native multiplexing

The problem

tmux (and zellij) are terminals-inside-terminals. They parse PTY output, maintain their own screen state, then re-encode escape codes for the outer terminal. This causes:
- Feature loss at the multiplexer boundary (graphics protocols, kitty keyboard, etc.)
- Escape code translation overhead and bugs
- TERM/terminfo configuration hacks
- The entire terminal ecosystem drags because new features need multiplexer buy-in

Ghostty's own split implementation delegates layout to platform widgets (AppKit NSSplitView, GTK GtkPaned), each with its own GPU surface. This causes visible flicker during resize because the
platform layout pass and GPU redraws are desynchronized.

The vision

A terminal emulator built on libghostty that:
- Provides all tmux features (and more) without the terminal-in-a-terminal architecture
- Uses single-surface GPU rendering for flicker-free split management
- Delivers a richer, more customizable UI than tmux's text-mode status bars and borders

Architecture

┌─────────────────────────────────────────────────┐
│              Single Metal/GPU Surface            │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐      │
│  │ Pane 1   │  │ Pane 2   │  │ Pane 3   │      │
│  │ libvt    │  │ libvt    │  │ libvt    │      │
│  │ instance │  │ instance │  │ instance │      │
│  └──────────┘  └──────────┘  └──────────┘      │
│  ┌──────────────────────────────────────┐       │
│  │ Status bar / tab bar / overlays      │       │
│  └──────────────────────────────────────┘       │
└─────────────────────────────────────────────────┘
         ▲                    ▲
         │ structured cell data (no re-encoding)
         │                    │
┌────────┴────────┐  ┌───────┴─────────┐
│ PTY 1 → libvt   │  │ PTY 2 → libvt   │  ...
└─────────────────┘  └─────────────────┘

Key design decisions:
- Single GPU surface -- all panes, dividers, overlays, and status bars rendered in one draw call. No platform widget resize race. No flicker.
- libghostty-vt per pane -- each pane owns a libghostty-vt instance that parses PTY bytes into structured cell/attribute data. No escape code re-encoding.
- Cell-grid-aware layout -- split boundaries snap to cell grid lines. Frame presentation is synchronized across all panes.
- Wezterm-style mux daemon (optional, for session persistence) -- headless server owns PTYs and libghostty-vt instances, GUI connects over Unix socket with structured protocol. Enables
detach/reattach without the tmux translation layer.

Features to port from your tmux config

┌──────────────────────────────────────────────────────────────────────────────────────────┬────────────────────────────────────────────────────────────────────────────────────────────────────┐
│                                         Feature                                          │                                      Implementation approach                                       │
├──────────────────────────────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ Prefix-key modal system (Ctrl-Space)                                                     │ Key tables / modal input layer. Ghostty 1.3 added key tables -- same concept, built natively       │
├──────────────────────────────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ Smart-splits (tmux ↔ neovim)                                                             │ Detect foreground process directly (no ps shell-out needed when you own the PTY)                   │
├──────────────────────────────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ Vim copy mode (v/V/C-v state machine, incremental search, OSC 133 prompt jump, search    │ Query libghostty-vt's scrollback buffer and cell grid directly. Build selection/search as a native │
│ match count)                                                                             │  UI overlay                                                                                        │
├──────────────────────────────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ Popup overlays (lazygit, lazydocker, fzf)                                                │ Native floating views composited in the single surface -- not nested terminals                     │
├──────────────────────────────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ Hint-copy (tmux-thumbs)                                                                  │ Pattern match against libghostty-vt's cell grid, render easymotion-style hint overlay              │
├──────────────────────────────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ URL picker (tmux-fzf-url)                                                                │ Extract URLs from scrollback via libghostty-vt, present in native picker                           │
├──────────────────────────────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ Sesh-like project sessions                                                               │ First-class workspace/project concept (directory-based, zoxide integration)                        │
├──────────────────────────────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ Theme via ANSI palette                                                                   │ Pass palette to renderer; theme changes are a palette swap, same as your current Ghostty setup     │
├──────────────────────────────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ Status bar (session name, zoom indicator, sysstat, clock)                                │ Custom-rendered bar in the single surface -- fully programmable, not limited to tmux's format      │
│                                                                                          │ strings                                                                                            │
├──────────────────────────────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ Session persistence (detach/reattach)                                                    │ Mux daemon approach (like wezterm-mux-server or zmx). Hardest feature -- defer to later            │
└──────────────────────────────────────────────────────────────────────────────────────────┴────────────────────────────────────────────────────────────────────────────────────────────────────┘

Prior art and resources

┌────────────────────────────────────────────────────┬─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│                      Project                       │                                                          Relevance                                                          │
├────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ https://github.com/ghostty-org/ghostling           │ Official minimal libghostty C API example -- your starting point                                                            │
├────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ https://libghostty.tip.ghostty.org/                │ 9 functional groups, 8 examples                                                                                             │
├────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ https://github.com/Uzaaft/awesome-libghostty       │ 40+ projects building on libghostty (language bindings, terminals, embeds)                                                  │
├────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ https://github.com/neurosnap/zmx                   │ Session persistence with libghostty-vt (~1k LoC, good reference for daemon approach)                                        │
├────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ wezterm source (mux/, codec/, wezterm-term/)       │ Production mux protocol implementation -- reference for structured diff, dirty line tracking, client-server protocol design │
├────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ https://github.com/coder/ghostty-web               │ libghostty-vt compiled to WASM with xterm.js compat -- shows the API in use                                                 │
├────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ https://github.com/nickel-lang/libghostty-rs       │ Rust bindings if you go that route                                                                                          │
├────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ https://mitchellh.com/writing/libghostty-is-coming │ Mitchell's blog post on the vision and API design                                                                           │
└────────────────────────────────────────────────────┴─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘

Suggested build sequence

1. Ghostling clone -- get a single-pane terminal rendering with libghostty-vt + Metal (macOS). Confirm the API, understand the cell grid, get font rendering working.
2. Single-surface split rendering -- two libghostty-vt instances, one Metal surface, manual layout. Resize without flicker.
3. Input layer -- prefix-key modal system, pane navigation, split create/close/zoom.
4. Copy mode -- vim motions over the scrollback buffer, visual selection, search with match counting, prompt jump.
5. Overlays -- popup panels (for lazygit etc.) and hint-copy as composited layers in the single surface.
6. Session/workspace model -- project-based sessions with directory association.
7. Mux daemon -- extract PTY + libghostty-vt ownership into a headless server for detach/reattach.

Risks

- libghostty-vt is alpha -- breaking API changes expected. You'll be tracking upstream.
- Rendering library not shipped -- the GPU rendering and widget layers from libghostty are planned but not available. You build your own renderer from the start.
- Single-surface compositing -- you lose platform accessibility, native drag handles, window management integration. All of that becomes your responsibility.
- Scope -- this is a full terminal emulator + multiplexer + custom UI framework. Steps 1-3 are a viable MVP; steps 4-7 are where it becomes a serious project.


