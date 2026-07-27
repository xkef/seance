# Clickable links through tmux and ripgrep

seance parses, dedupes, hover-highlights, and click-opens OSC 8 hyperlinks. The
full "click a `rg` hit and jump the editor to the line" chain has three links
beyond seance's own detection, and this page walks through wiring them up.

This is the end-to-end recipe tracked by [#253].

## The chain

1. The shell (or a tool like `rg`) emits an OSC 8 hyperlink whose URL carries a
   `:LINE:COL` location anchor.
2. tmux forwards OSC 8 to the outer terminal — but only if it believes the outer
   terminal supports hyperlinks.
3. seance detects the link, parses the location anchor, and hands
   `(path, line, col)` to a handler that launches the editor at the right spot.

Each step below closes one gap.

## 1. Advertise hyperlink support to tmux

tmux strips OSC 8 unless the outer terminal's entry lists the `hyperlinks`
feature. Install the bundled seance terminfo, which declares truecolor (`Tc`),
styled/colored underlines (`Su` / `Smulx`), DECSCUSR cursor shaping, and
synchronized output on top of `xterm-256color`:

```sh
tic -x tools/seance.terminfo
```

Point tmux at it and tell tmux the entry supports hyperlinks:

```tmux
set -g default-terminal "xterm-seance"
set -as terminal-features "xterm-seance:hyperlinks:RGB"
```

Verify:

```sh
infocmp -x xterm-seance | grep -E 'Su|Tc'
tmux show -g terminal-features | grep hyperlinks
```

> Switching seance's advertised `TERM` from `xterm-256color` to `xterm-seance`
> by default (with a fallback when `tic` is unavailable) is tracked as the
> remaining half of [#253]; until then, set `TERM`/`default-terminal` yourself
> as above.

## 2. Make ripgrep emit location anchors

```sh
rg --hyperlink-format=default 'needle'
```

`--hyperlink-format=default` produces `file:///abs/path:LINE:COL` URLs. Wrap it
in your shell config so every `rg` invocation is clickable:

```fish
# ~/.config/fish/config.fish
alias rg 'rg --hyperlink-format=default'
```

seance understands the location anchors these tools emit:

| URL                     | Opens at  |
| ----------------------- | --------- |
| `file:///abs/path`      | line 1    |
| `file:///abs/path:42`   | line 42   |
| `file:///abs/path:42:7` | line 42:7 |
| `file:///abs/path#42`   | line 42   |
| `file:///abs/path#L42`  | line 42   |

## 3. Route `file://` links to your editor

By default seance hands links to the platform opener (`open` on macOS,
`xdg-open` on Linux), which ignores the anchor and opens the file at the top.
Configure a per-scheme handler in `seance.toml` to launch an editor at the
matching line instead:

```toml
[links.handlers]
# Glob is matched against the URL with its location anchor stripped, so the
# extension glob still matches file:///…/foo.rs:42:7.
"file://*.{rs,toml,md}" = ["nvim", "+{line}", "{path}"]
"https://*"             = "open"
```

Placeholders substituted into each argument:

- `{url}` — the full link URL
- `{path}` — the filesystem path (anchor stripped)
- `{line}` — the line number, when present
- `{col}` — the column, when present

An argument that references `{line}` or `{col}` while that anchor is absent is
dropped, so `"+{line}"` disappears rather than becoming a bare `"+"`. A handler
with no placeholder at all (for example the bare `"open"` above) gets the URL
appended as a trailing argument. Handler globs support `*` and `{a,b,c}`
alternation. The first matching entry (in the map's sorted order) wins; with no
match, seance falls back to the platform opener.

## Verify the whole chain

Inside tmux, run `rg --hyperlink-format=default` for a term you know appears in
a source file, then modifier-click a hit (the modifier is `[links].modifiers`,
default `super+shift` on macOS and `ctrl+shift` elsewhere). Your configured
editor should open the file at the matching line.

[#253]: https://github.com/xkef/seance/issues/253
