---
name: seance-fidelity
description: Capture Ghostty and seance with matched theme, font, padding, and fixture content, then compare screenshots and metrics. Use when debugging renderer fidelity issues such as xkef/seance#19.
---

# Seance Fidelity

Use this skill for visual diffs between Ghostty and `seance`, especially when the
likely causes are renderer math rather than config drift.

Read `tools/fidelity/README.md` if the harness behavior or environment variables
matter for the current task.

## Workflow

1. Prefer an image-capable model.
2. Run the capture harness:
   ```bash
   bash tools/fidelity/capture_pair.sh
   ```
3. Read these outputs:
   - `tools/fidelity/artifacts/latest/ghostty.png`
   - `tools/fidelity/artifacts/latest/seance.png`
   - `tools/fidelity/artifacts/latest/diff.png`
   - `tools/fidelity/artifacts/latest/metrics.json`
   - `tools/fidelity/artifacts/latest/ghostty.config`
   - `tools/fidelity/artifacts/latest/seance.toml`
4. If the diff points at a renderer bug, inspect the most relevant code and make
   a focused change.
5. Re-run the harness after each change until the delta is fixed or clearly
   isolated.
6. Finish with a short correctness report. If the remaining deltas deserve more
   work, file follow-up issues with `gh`.

## Likely code paths for issue #19

- `crates/seance-render/src/gpu/shaders/cell.wgsl`
- `crates/seance-render/src/gpu/uniforms.rs`
- `crates/seance-render/src/text/cell_builder.rs`
- `crates/seance-render/src/renderer.rs`
- `crates/seance-app/src/main.rs`

## Heuristics

- Global light/dark drift across anti-aliased text often points at
  sRGB-vs-linear mistakes or premultiplication/blend math.
- Deltas concentrated in the low-contrast fixture rows point at
  min-contrast handling.
- Deltas concentrated in the `bold` ANSI row point at `bold-is-bright`
  handling.
- Deltas visible only when `FIDELITY_BACKGROUND_OPACITY` is below `1.0`
  point at background alpha/compositing.
- Prefer fixing one suspected cause at a time. Re-capture after each change.

## Reporting

Summarize:

- the exact capture settings used
- what changed numerically in `metrics.json`
- which fixture rows improved or still differ
- whether the remaining delta is acceptable or needs a follow-up issue
