# séance

A GPU-rendered terminal aiming at built-in multiplexing — tabs, splits, and
persistent sessions, **without a terminal inside a terminal**. macOS-first;
Linux is a target.

Built on [`libghostty-vt`][libghostty] (Ghostty's terminal core, Rust bindings)
and [`wgpu`][wgpu]. No hand-rolled VT parser. No bespoke graphics layer.

> **Status:** early. The single-pane terminal renders and runs a shell. The
> native multiplexer that motivates the name (M6) is planned, not built.

## Build & run

```sh
tools/setup-ghostty-src.sh   # clone + patch vendored ghostty-src
tools/setup-sysroot.sh       # macOS 26.4 SDK overlay for Zig's arm64 linker
tools/run.sh                 # build and launch
```

## Docs

- Architecture & subsystem status:
  [`docs/architecture.md`](docs/architecture.md)
- Naming (`séance` vs `seance`): [`docs/naming.md`](docs/naming.md)
- Roadmap: GitHub epics M1–M11 (`label:epic`)

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your
option.

[libghostty]: https://github.com/Uzaaft/libghostty-rs
[wgpu]: https://wgpu.rs
