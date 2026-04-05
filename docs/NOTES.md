# Ghostty-based Terminal Multiplexer — Project Summary & Roadmap

## Vision

A GPU-rendered, plugin-based terminal multiplexer built on the libghostty ecosystem. Built-in multiplexing replaces tmux without the terminal-within-a-terminal overhead. A Lua plugin system lets tools like fuzzy finders and prompt widgets break out of the terminal grid into rich GPU-rendered overlays. Deep Neovim integration via RPC makes editor and terminal feel like one tool.

**One-liner pitch:** Ghostty's terminal emulation + built-in multiplexing + Lua plugins. No tmux needed.

---

## Why libghostty

libghostty-vt is a zero-dependency C library extracted from Ghostty. It handles VT sequence parsing, terminal state (cursor, styles, reflow, scrollback), input encoding (Kitty keyboard protocol, mouse), and provides a high-performance render state API — but has no opinion on rendering or GUI. The Rust bindings (`libghostty-vt` crate from `libghostty-rs`) wrap the C API with safe Rust types: `Terminal`, `RenderState`, `KeyEncoder`, `MouseEncoder`, `RowIterator`, `CellIterator`.

### Why not a standalone renderer

Ghostty's renderer (Zig, Metal + OpenGL) is battle-tested and handles all the hard text rendering: glyph atlas, ligatures, emoji, wide chars, font fallback, underline styles, Kitty graphics protocol, synchronized rendering. Mitchell Hashimoto has explicitly outlined plans for a `libghostty-renderer` library that would let consumers hand it a GPU surface and get Ghostty-quality rendering for free. Building the multiplexer on top of this future library means:

- Phase 1 shrinks from "build a terminal renderer" to "wire up multiplexer logic"
- Ghostty-quality rendering without reimplementing it
- Automatic compatibility with Ghostty themes, config, and shader format
- Project positioned as part of the Ghostty ecosystem, not competing with it

### Why multiplexing is the right gap

Ghostty explicitly does not build multiplexing — it's out of scope. tmux's fundamental problem is being a terminal-within-a-terminal: it re-parses VT sequences and re-emits them, losing fidelity (true color mangling, Kitty keyboard protocol eaten, image protocols broken). With libghostty-vt, each pane gets its own `Terminal` instance and PTY, rendered directly to GPU. No intermediate encoding, no information loss.

---

## Architecture

```
┌──────────────────────────────────────────────────┐
│  Lua config/plugins (mlua)                       │
│  keybinds, layouts, hooks, prompt, fuzzy finder  │
├──────────────────────────────────────────────────┤
│  Multiplexer core (Rust)                         │
│  pane management, tiling layout, sessions        │
├───────────────────┬──────────────────────────────┤
│  Terminal panes   │  Neovim panes                │
│  PTY + libghostty │  nvim --embed + RPC          │
│  -vt per pane     │  via nvim-rs                 │
├───────────────────┴──────────────────────────────┤
│  libghostty-renderer (future)                    │
│  GPU rendering: cell grid, overlays, cursors     │
├──────────────────────────────────────────────────┤
│  winit (windowing / input)                       │
└──────────────────────────────────────────────────┘
```

### Two pane types

1. **Terminal pane** — PTY + libghostty-vt `Terminal` instance. Raw shell interaction.
2. **Neovim pane** — `nvim --embed`, headless Neovim sending structured UI events via msgpack RPC. Renders through the same cell renderer but with richer overlays for completion, hover docs, floating windows.

---

## Core Stack

| Component         | Crate / Library              | Purpose                                   |
|--------------------|------------------------------|-------------------------------------------|
| VT parsing         | `libghostty-vt`              | Terminal state, VT sequences, input encoding |
| Rendering          | `libghostty-renderer` (future) | GPU cell rendering, glyph atlas, text shaping |
| Windowing          | `winit`                      | Window creation, input events             |
| Lua runtime        | `mlua`                       | Config, keybindings, plugin system        |
| Neovim RPC         | `nvim-rs`                    | Embedded Neovim communication             |
| Fuzzy matching     | `nucleo`                     | Fuzzy finder engine (same as Helix)       |
| PTY                | `portable-pty`               | PTY spawning and management               |
| Serialization      | `serde` + messagepack/JSON   | Session save/restore                      |

---

## Ghostty Ecosystem Integration

Maximize benefit from Ghostty's momentum and community:

- **Use libghostty-vt** (and future libghostty-renderer) — ties project to Ghostty brand
- **Parse Ghostty config format** for terminal settings (font, theme, colors) — users drop in existing config
- **Load Ghostty theme files** directly — instant access to hundreds of themes
- **Reuse Ghostty's shell integration scripts** — they emit OSC 133 / OSC 7, consumed identically
- **Support Ghostty's custom shader format** if feasible
- **Contribute renderer extraction upstream** — shape the libghostty-renderer API to fit multiplexer needs, become a core contributor

### Community strategy

- Join Ghostty Discord `#libghostty` channel
- Introduce project concept, get feedback on design
- Ask about `libghostty-renderer` timeline
- Offer to help extract the renderer as a reusable C library (the API Mitchell wants to exist)
- Position as "the tmux replacement for Ghostty users"

---

## The Lua Layer

Everything above the terminal core is configured and extended with Lua. No custom config DSL.

### Multiplexer config

```lua
local term = require("term")

term.on_startup(function()
  local main = term.pane.current()
  local side = main:split_right({ ratio = 0.3 })
  side:split_bottom({ ratio = 0.5 })
  main:run("nvim .")
  side:run("lazygit")
end)

term.keymap.set("ctrl+a", "leader")
term.keymap.set("leader h", term.pane.focus_left)
term.keymap.set("leader v", function()
  term.pane.current():split_right()
end)
```

### Plugin hooks

| Hook                  | Fires when                                   |
|------------------------|----------------------------------------------|
| `on_prompt`            | OSC 133 prompt marker detected               |
| `on_output_start/end`  | Command output brackets                      |
| `on_key`               | Keystroke before PTY, return if consumed      |
| `on_directory_change`  | OSC 7 working directory change               |
| `render_overlay`       | Plugin gets to draw GPU overlay              |
| `render_prompt`        | Plugin replaces prompt with custom widget    |
| `provide_completions`  | Plugin returns fuzzy-matched items           |

### API namespaces

| Namespace      | Exposes                                                   |
|----------------|-----------------------------------------------------------|
| `term.pane`    | create, split, close, focus, resize, serialize/restore    |
| `term.keymap`  | leader keys, chords, modal maps, per-pane overrides       |
| `term.ui`      | overlays, pickers, prompt widgets, notifications          |
| `term.history` | shell history access                                      |
| `term.git`     | repository state                                          |
| `term.nvim`    | Neovim RPC bridge                                         |
| `term.hook`    | register callbacks for terminal events                    |

---

## Neovim Integration

### Phase 1 — RPC to running instances (lightweight)

- Detect Neovim in PTY panes via `$NVIM` env var
- Auto-connect RPC to running `nvim` instances via `v:servername`
- Seamless pane/split navigation — `Ctrl+H/J/K/L` moves between terminal panes and Neovim splits without vim-tmux-navigator hacks:

```lua
term.keymap.set("ctrl+h", function(pane)
  if pane:is_neovim() then
    pane:nvim():cmd("wincmd h")
  else
    term.pane.focus_left()
  end
end)
```

- Click file paths in terminal output → opens in Neovim via RPC
- Unified theming — read Neovim's colorscheme, apply everywhere

### Phase 2 — Embedded Neovim (full integration)

- `nvim --embed` as a pane type, communicating via msgpack RPC
- Call `nvim_ui_attach()` with `ext_multigrid`, `ext_popupmenu`, `ext_cmdline`
- Neovim sends structured UI events (`grid_line`, `hl_attr_define`, `popupmenu_show`, etc.) instead of escape sequences
- Render Neovim's grid through the same GPU pipeline as terminal panes
- Completion menus, hover docs, floating windows rendered as native GPU overlays with transparency, shadows, animations
- No escape sequence round-trip, no fidelity loss

---

## Built-in Plugin Ideas

### Fuzzy finder (replaces fzf)

Intercept `Ctrl+R` / `Ctrl+T` at terminal level. Render as a GPU floating overlay (Spotlight/Raycast style) instead of a TUI. Use `nucleo` for matching. Data sources: shell history (read `~/.zsh_history`), file tree (`ignore` crate), git refs, zoxide database, atuin history DB.

### Prompt widget (replaces starship in-terminal)

Detect prompt via OSC 133. Replace with GPU-rendered segments — path, git branch, dirty state, language versions. Use Starship as a Rust library for data collection, render with your own widget system.

### Path clicker

Parse file paths + line numbers from terminal output (compiler errors, `rg` results, stack traces). Click to open in Neovim pane via RPC.

---

## Roadmap

### Phase 0 — Community & Design (now, 1-2 weeks)

- [ ] Join Ghostty Discord `#libghostty`
- [ ] Introduce project concept, get feedback
- [ ] Ask about `libghostty-renderer` timeline and offer to help extract it
- [ ] Write design doc / RFC for the Lua plugin API boundaries
- [ ] Evaluate current state of libghostty-vt Rust bindings
- [ ] Pick a name

### Phase 1 — Single pane terminal (2-4 weeks if libghostty-renderer exists, 2-3 months if building renderer)

- [ ] winit window + libghostty-renderer (or fallback: basic wgpu cell renderer)
- [ ] libghostty-vt `Terminal` + `RenderState` driving the grid
- [ ] PTY spawning via `portable-pty`, read/write threads
- [ ] Keyboard input via `KeyEncoder`, mouse input
- [ ] Scrollback, selection, clipboard
- [ ] Load Ghostty theme files and config
- **Goal:** daily-drivable single-pane terminal

### Phase 2 — Multiplexer + Lua (3-6 weeks)

- [ ] Tiling layout engine (hsplit, vsplit, resize)
- [ ] Multiple `Terminal` instances, one per pane
- [ ] Pane focus, navigation, close, zoom
- [ ] `mlua` integration — keybindings and layout as Lua
- [ ] Named sessions with serialize/restore
- [ ] Project-based auto-layouts (detect repo type, apply layout)
- **Goal:** replaces tmux for daily workflow

### Phase 3 — Plugin system (4-8 weeks)

- [ ] Hook system: `on_prompt`, `on_key`, `on_directory_change`, `on_output_end`
- [ ] `term.ui` overlay API (GPU-rendered floating panels)
- [ ] `term.pane` / `term.keymap` APIs exposed to Lua
- [ ] Built-in fuzzy picker using `nucleo`
- [ ] Prompt widget with Starship data
- **Goal:** fzf / starship functionality as native Lua plugins

### Phase 4 — Neovim RPC integration (2-3 weeks)

- [ ] Detect Neovim in PTY panes, auto-connect RPC via `nvim-rs`
- [ ] Seamless split navigation across terminal and Neovim panes
- [ ] Click file paths → open in Neovim
- [ ] Unified theming
- **Goal:** terminal and editor feel like one tool

### Phase 5 — Embedded Neovim (4-6 weeks)

- [ ] `nvim --embed` as a pane type
- [ ] Render Neovim UI events through GPU pipeline
- [ ] Rich overlays for completion, hover docs, floating windows
- [ ] Handle `:terminal` buffers inside embedded Neovim
- **Goal:** Neovide-quality rendering inside the multiplexer

### Rule: don't start the next phase until the current one is your daily driver

---

## Key Design Decisions to Make Early

1. **libghostty-renderer availability** — if it's months away, decide whether to build a basic renderer or wait. Building basic + swapping later is viable.
2. **Plugin API boundaries** — the hook signatures and `term.ui` widget model are hard to change later. Get feedback from Ghostty Discord before locking these in.
3. **Neovim pane type** — decide early if embedded Neovim is opt-in or default. Start opt-in.
4. **Config story** — Ghostty config for terminal settings + Lua for multiplexer/plugins, or all Lua? Ghostty config compat is a strong selling point.
5. **Daemon mode** — do panes survive terminal close (like tmux server)? This affects architecture from the start.

---

## References

- **libghostty C API docs:** <https://libghostty.tip.ghostty.org/>
- **libghostty-rs (Rust bindings):** <https://github.com/Uzaaft/libghostty-rs>
- **ghostling (minimal C terminal):** <https://github.com/ghostty-org/ghostling>
- **ghostling_rs (minimal Rust terminal):** in libghostty-rs repo under `example/ghostling_rs`
- **Neovim UI protocol:** `:help ui.txt` in Neovim
- **nvim-rs crate:** <https://crates.io/crates/nvim-rs>
- **nucleo (fuzzy matching):** <https://crates.io/crates/nucleo>
- **Neovide (embedded Neovim reference):** <https://github.com/neovide/neovide>
- **Ghostty Discord:** <https://discord.gg/ghostty> (look for `#libghostty` channel)
- **Mitchell's libghostty announcement:** <https://mitchellh.com/writing/libghostty-is-coming>
- **awesome-libghostty:** <https://github.com/Uzaaft/awesome-libghostty>
