# Fidelity capture harness

macOS-only screenshot workflow for comparing `seance` against Ghostty with the
same fixture, theme, font, padding, and window bounds.

## Files

- `fixture.sh` — deterministic terminal content for issue #19
- `capture_pair.sh` — builds `seance`, launches Ghostty and `seance`
  sequentially, captures screenshots, and runs image diffing
- `compare.py` — PNG diff + JSON metrics
- `window_info.swift` — queries macOS window IDs/bounds by PID

## Basic usage

```sh
bash tools/fidelity/capture_pair.sh
```

Outputs land in `tools/fidelity/artifacts/latest/`:

- `ghostty.png`
- `seance.png`
- `diff.png`
- `metrics.json`
- `ghostty.config`
- `seance.toml`
- `capture.env`
- `ghostty.window.json`
- `seance.window.json`

## Common overrides

```sh
FIDELITY_THEME='Gruvbox Dark' \
FIDELITY_PROFILE=dev \
FIDELITY_BOUNDS='140,120,1100,760' \
FIDELITY_BACKGROUND_OPACITY=0.85 \
bash tools/fidelity/capture_pair.sh
```

Supported environment variables:

- `FIDELITY_PROFILE` — cargo profile passed to `tools/make-app.sh`
- `FIDELITY_OUT_DIR` — output directory
- `FIDELITY_THEME`
- `FIDELITY_FONT_FAMILY`
- `FIDELITY_FONT_SIZE`
- `FIDELITY_MIN_CONTRAST`
- `FIDELITY_ADJUST_CELL_HEIGHT`
- `FIDELITY_BOLD_IS_BRIGHT`
- `FIDELITY_PADDING_X`
- `FIDELITY_PADDING_Y`
- `FIDELITY_DECORATION`
- `FIDELITY_BACKGROUND_OPACITY`
- `FIDELITY_CURSOR_STYLE`
- `FIDELITY_CURSOR_BLINK`
- `FIDELITY_BOUNDS` — `x,y,width,height`
- `FIDELITY_SETTLE_SECONDS`
- `FIDELITY_FIXTURE`
- `FIDELITY_GHOSTTY_APP`
- `FIDELITY_SEANCE_APP`
- `FIDELITY_SEANCE_BIN`

## Notes

- Ghostty and `seance` are captured **sequentially at the same window
  position**. That makes transparent-background comparisons less noisy than
  side-by-side captures.
- The harness uses `screencapture -l <window-id>`, so overlapping windows are
  less of a problem than region captures.
- The default config disables decorations to avoid titlebar/compositor noise.
  Re-enable them with `FIDELITY_DECORATION=true` if the issue requires it.
- `osascript` needs Accessibility permission to move/focus windows.
- The diff image amplifies per-channel deltas. `metrics.json` contains numeric
  summaries plus the hottest changed tiles.
