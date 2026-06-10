# Threading

This document is the architectural decision record for séance's M2 threading
model. It supersedes the earlier plan that attempted to make
`libghostty_vt::Terminal` movable across threads. Upstream `libghostty-rs` keeps
`Terminal` `!Send`; séance's design follows that constraint instead of working
around it.

[#45]: https://github.com/xkef/seance/issues/45
[#171]: https://github.com/xkef/seance/issues/171
[#172]: https://github.com/xkef/seance/issues/172
[#221]: https://github.com/xkef/seance/issues/221
[#222]: https://github.com/xkef/seance/issues/222
[#23]: https://github.com/xkef/seance/issues/23
[#24]: https://github.com/xkef/seance/issues/24
[#26]: https://github.com/xkef/seance/issues/26

---

## Status

| Concern                | v1 implementation                                         |
| ---------------------- | --------------------------------------------------------- |
| PTY blocking `read`    | one Unix VT Actor owns PTY read/write/readiness polling   |
| VT parser (`vt_write`) | VT Core on the actor thread only                          |
| libghostty ownership   | VT Core owns `Terminal`, `RenderState`, and Kitty setup   |
| UI read path           | UI reads mux materialized pane snapshots only             |
| UI write path          | `seance-mux-client::Pane` commands over local VT session  |
| Dirty reset            | render-generation acknowledgement after successful render |
| Wake from IO           | pane-scoped mux event after snapshot publish              |
| DEC 2026 sync output   | VT Actor suppresses publishes while sync mode is active   |
| Renderer thread        | none; revisit only after profiling                        |
| Platform scope         | actor v1 is Unix-only while raw-fd polling is used        |

If a feature branch contains the obsolete `Terminal: Send` experiment or a
forked `libghostty-rs` dependency for thread-safety, treat that code as
superseded. Useful pieces may be retained only when they still fit this actor
model, such as UI-owned selection state and PNG decoder installation inside VT
Core on the owning VT Actor thread.

---

## Decision

**All `libghostty-vt` objects are created, used, and dropped inside VT Core on
one VT Actor thread.**

The UI thread never owns, locks, borrows, moves, or references:

- `libghostty_vt::Terminal`
- `libghostty_vt::RenderState`
- libghostty iterators or snapshots
- raw references into libghostty state

The only shared VT data is séance-owned, immutable snapshot data. Phase 1 adds
`seance-mux-client` between the app and `seance-vt`; the mux materializes
ordered pane updates but does not share live VT state.

```
┌─ UI thread (winit + wgpu) ───────────────┐   ┌─ VT Actor thread ───────────────┐
│ owns App, SurfaceState, renderer         │   │ owns VT Core                    │
│ owns PaneView + selection                │   │   owns libghostty Terminal      │
│ reads materialized VtSnapshot            │   │   owns persistent RenderState    │
│ renders SnapshotFrameSource              │   │   extracts owned VtSnapshots     │
│ acks presented snapshot generation       │   │ owns PTY master/reader/writer    │
│ encodes input using snapshot modes       │   │ nonblocking poll loop            │
│                                          │   │ handles VtCommand                │
│ seance-mux-client::Pane ─── VtCommand ─────────▶│   │ publishes to SnapshotSlot        │
│ EventLoopProxy ◀──── MuxEvent ───────────│   │ dedupes ContentDirty wakes       │
└──────────────────────────────────────────┘   └────────────────────────────────┘
```

Do **not** implement `Arc<Mutex<libghostty_vt::Terminal>>` or any equivalent
shared live VT state. The mutex/lock discipline from the previous plan is no
longer the contract.

---

## Snapshot model

The VT Actor publishes owned snapshots. The UI clones only the `Arc`; it does
not clone per-cell text or image payloads during normal rendering.

```rust
pub struct VtSnapshot {
    pub generation: u64,
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<SnapshotCell>, // row-major, len = cols * rows
    pub text: String,             // arena for non-empty cell text
    pub cursor: CursorInfo,
    pub modes: TerminalModes,
    pub dirty: DirtySnapshot,
    pub placements: Vec<PlacementSnapshot>,
    pub images: Vec<SnapshotImage>,
}

pub struct SnapshotCell {
    pub text_start: u32,
    pub text_len: u16,
    pub fg: CellColor,
    pub bg: CellColor,
    pub attrs: CellAttrs,
}

pub struct SnapshotImage {
    pub image_id: ImageId,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
```

VT Snapshots carry row-dirty information by render generation:

- `generation` increases for every successful VT Snapshot extraction.
- `dirty` is `DirtySnapshot::Full` initially.
- VT Core records dirty deltas per generation and publishes the union of
  unacknowledged deltas in each VT Snapshot.
- A pane view acknowledges the generation after the renderer successfully
  presents the VT Snapshot it used.
- Stale acknowledgements never clear newer dirty rows. Over-upload is
  acceptable; under-upload is not.
- Cell text uses one arena string plus offsets; avoid `String` allocation per
  cell.
- `CellColor` stays symbolic (`Default`, `Palette`, `Rgb`), not resolved RGBA,
  so theme reload can repaint from the same snapshot.

Snapshot extraction happens inside VT Core from a coherent
`libghostty_vt::RenderState` snapshot and its row/cell iterators.

### VT Core

VT Core is the only Module that owns live libghostty state. It hides:

- `libghostty_vt::Terminal`
- `libghostty_vt::RenderState`
- libghostty render snapshots and iterators
- Kitty graphics setup and PNG decoder installation
- cell-pixel tracking for image placement
- cursor and theme seeding
- VT Snapshot extraction errors
- dirty-row generation tracking

VT Actor wraps VT Core for PTY I/O and command polling. Headless VT wraps VT
Core without a PTY so Layer 4 tests exercise the same VT Snapshot extraction
path as production.

### Cursor and modes

Cursor data must come from the libghostty render snapshot, not direct terminal
cursor fields:

- `snapshot.cursor_visible()`
- `snapshot.cursor_viewport()`
- `snapshot.cursor_visual_style()`

The snapshot also copies terminal modes needed by UI input encoding:

- application cursor keys
- mouse tracking
- SGR mouse format
- bracketed paste

One-frame-stale mode reads are acceptable for v1.

### Selection text

Selection rectangle state remains UI-owned. Text extraction moves to
`VtSnapshot` so copy operations never read live VT state.

Preserve the existing behavior exactly:

- normalize the selected range before iterating
- line selection selects whole rows
- an empty selected cell contributes `' '`
- trim trailing whitespace after each selected row
- return `None` if the final result is empty

### Kitty graphics

Kitty graphics are part of the v1 snapshot for correctness. The protocol seam
also defines ordered image cache events for remote transports.

- `VtSnapshot::visit_images()` yields borrowed image info from `SnapshotImage`
  on the current in-process compatibility path.
- `VtSnapshot::visit_placements(layer)` filters placement snapshots by layer.
- `PaneUpdate.image_events` apply before the frame in the same ordered update.
- Remote image payloads use `ImageCacheEvent` puts/evicts keyed by
  `ImageKey { pane, image_id }`.
- Renderer LRU eviction is local. A missing referenced image is skipped, not a
  panic or server eviction.

---

## FrameSource adapter

`VtSnapshot` is immutable. The renderer sees it through a small adapter:

```rust
pub struct SnapshotFrameSource<'a> {
    snapshot: &'a VtSnapshot,
}
```

`SnapshotFrameSource` implements `FrameSource`:

- `grid_size`
- `cursor`
- `visit_cells`
- `dirty_rows` by cloning `snapshot.dirty`
- `clear_dirty` as a no-op; dirty reset is `ack_rendered(generation)`
- `visit_images`
- `visit_placements`

Do not implement `FrameSource` directly on `Arc<VtSnapshot>`. Do not expose a
live libghostty `FrameSource` adapter as a public compatibility path.

---

## Snapshot publication

Keep the snapshot slot behind a small API instead of exposing a raw
`Arc<Mutex<Option<Arc<VtSnapshot>>>>` through app code.

```rust
#[derive(Clone)]
pub struct SnapshotSlot {
    inner: Arc<Mutex<Option<Arc<VtSnapshot>>>>,
}
```

`VtSessionHandle` owns the slot and exposes:

```rust
pub fn latest_snapshot(&self) -> Option<Arc<VtSnapshot>>;
pub fn clear_content_dirty_pending(&self);
pub fn ack_rendered(&self, generation: u64) -> Result<(), VtSessionError>;
```

Wake ordering:

1. VT Actor builds an owned `VtSnapshot`.
2. VT Actor publishes it to `SnapshotSlot`.
3. VT Actor sends/dedupes `VtEvent::ContentDirty`.
4. `LocalDomain` handles `ContentDirty`, clears the pending flag before cloning
   the latest snapshot, materializes a Pane Update, and forwards a pane-scoped
   dirty wake to the app.

Clearing before the clone prevents a publish that races with UI handling from
being lost behind an already-set pending flag. Extra wakes are acceptable;
missed wakes are not.

Acknowledgement ordering:

1. The renderer builds and presents a frame from a VT Snapshot.
2. If present succeeds, the Pane Handle sends `AckRendered(generation)` through
   `MuxClient` and `LocalDomain` for that VT Snapshot.
3. VT Core drops dirty deltas for generations `<= generation`.
4. VT Actor does not publish solely because of an acknowledgement.

Dirty rows are never cleared when `ContentDirty` is handled and are never
cleared on snapshot publication.

---

## Mux protocol seam

`seance-mux-client` is the app-facing mux layer. `LocalDomain` owns the
`VtSessionHandle`, stamps a `PaneRef` onto local events, and produces ordered
Pane Updates. `MuxClient` materializes `FrameDelta` values into client-side Pane
Views and exposes app-facing Pane Handles. `seance-app` no longer imports
`seance-vt` directly.

The production single-process local path uses `LocalDomain` directly instead of
serializing local commands as a ritual. The Mux Protocol path lives in
`ProtocolDomain`, which implements the same Domain seam over a `Transport` using
length-prefixed postcard envelopes. Client commands encode as `ClientMessage`
values, and server pane updates encode as `ServerMessage::PaneUpdate` before a
Pane View materializes them. This keeps future UDS/SSH/TLS transports on the
same Domain Interface without making the local path pay for transport framing.

The local materializer distinguishes three counters:

- VT extraction `generation`, assigned by VT Core snapshots.
- Server-side `ServerSeq`, assigned by the mux to ordered pane updates.
- Renderer-presented generation, acknowledged after a successful present.

`FrameDelta::Full` carries a full `VtSnapshot`. `FrameDelta::Partial` carries
`base_generation`, new `generation`, dimensions, cursor, modes, placements, and
row-local `RowDelta` values. Applying a partial rewrites the materialized text
arena so all `SnapshotCell.text_start` offsets remain valid. If the base
generation or dimensions do not match, the client returns `NeedFull` and uses a
full reset.

The per-pane history ring retains ordered `PaneUpdate`s plus the latest full
update for first attach and resume. Reconnects replay retained updates from the
ring when possible; otherwise the mux sends a resync/full reset. See
[`docs/protocol.md`](./protocol.md) for envelope, image cache, flow-control, and
error taxonomy details.

---

## VT Actor API

`seance-vt` remains winit-free. The actor receives an event sink closure, which
`seance-app` can adapt to `EventLoopProxy<UserEvent>`.

```rust
pub fn spawn_vt_session<F>(
    options: VtSessionOptions,
    event_sink: F,
) -> Result<VtSessionHandle, SpawnError>
where
    F: Fn(VtEvent) + Send + 'static;
```

`spawn_vt_session` blocks until the VT Actor thread has successfully:

1. installed the PNG decoder for that thread
2. created VT Core, including the libghostty terminal, persistent render state,
   Kitty graphics setup, and cursor seeding
3. opened the PTY and spawned the child process
4. published the initial snapshot

VT Core creation must happen inside the VT Actor thread closure, not before
spawning. The initialization result is returned to the caller through an
internal one-shot or synchronous channel.

### Events

Keep public actor events minimal for the threading migration:

```rust
pub enum VtEvent {
    ContentDirty,
    Exited,
}
```

Do not add `PaneId` routing, bell events, title events, or clipboard events in
this migration.

`ContentDirty` is deduped with an atomic pending flag. If one content-dirty wake
is already in flight, the VT Actor only updates the snapshot slot.

### Commands

Use one simple `std::sync::mpsc::Sender<VtCommand>` for v1. The VT Actor drains
and coalesces command classes internally.

```rust
pub enum VtCommand {
    Write(bytes::Bytes),
    Resize {
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    },
    SetThemeColors(ThemeColors),
    ScrollLines(i32),
    SetCursorShape(CursorShape),
    AckRendered(u64),
    Shutdown,
}
```

Drain behavior:

- keep all `Write` payloads in order
- coalesce `Resize` to the latest value
- coalesce `SetThemeColors` to the latest value
- accumulate `ScrollLines` deltas
- coalesce `SetCursorShape` to the latest value
- coalesce `AckRendered` to the max generation
- exit promptly on `Shutdown`

Apply drained commands in deterministic order:

1. render-generation acknowledgement
2. resize
3. theme and cursor changes
4. scroll
5. writes

Publish a snapshot after every VT-visible mutation:

- PTY data parsed into the terminal
- resize
- theme colors
- cursor shape seed/change
- scroll viewport

Do not publish after `Write` alone; shell echo or output will publish after PTY
data arrives. Do not publish after `AckRendered` alone.

### Command errors

`VtSessionHandle` command methods return `Result<(), VtSessionError>`:

```rust
pub fn write(&self, bytes: bytes::Bytes) -> Result<(), VtSessionError>;
pub fn resize(&self, resize: Resize) -> Result<(), VtSessionError>;
pub fn scroll_lines(&self, delta: i32) -> Result<(), VtSessionError>;
pub fn ack_rendered(&self, generation: u64) -> Result<(), VtSessionError>;
```

App callsites may explicitly ignore or log these errors. `Drop` ignores errors.

### Shutdown

`VtSessionHandle` owns the VT Actor thread join handle internally. `Drop` sends
`Shutdown` and notifies the poller, but must not block waiting for the thread to
exit. Add an explicit `join(self)` for tests and controlled shutdown paths.

---

## Nonblocking Unix IO loop

The first actor implementation is Unix-only (`macOS` and Linux). Non-Unix
platforms should return `SpawnError::UnsupportedPlatform` until a non-raw-fd
readiness strategy exists.

Use:

- a private PTY Adapter Seam inside `seance-vt`
- `portable-pty` reader/writer objects for the production Adapter
- a scripted test Adapter for deterministic actor command/publication tests
- `MasterPty::as_raw_fd()` only for readiness polling in the production Adapter
- `O_NONBLOCK` on the master fd
- `polling::Poller::notify()` for command wakeups

Expected ownership shape:

```rust
let mut reader = master.try_clone_reader()?;
let mut writer = master.take_writer()?;
let fd = master.as_raw_fd().ok_or(SpawnError::NoRawFd)?;
set_nonblocking(fd)?;
```

### Pending writes

Use `bytes::Bytes` for public write payloads and pending write storage. Add
`bytes = "1"` to workspace dependencies and `bytes.workspace = true` to
`seance-vt` when implementing the actor.

```rust
struct PendingWrites {
    queue: VecDeque<bytes::Bytes>,
}
```

Flush behavior:

- write `front.chunk()`
- on `Ok(n)`, call `front.advance(n)`
- pop empty chunks
- stop on `WouldBlock`
- continue on `Interrupted`
- return other errors

Register writable poll interest only while the queue is non-empty.

### Read/parse budget

Bound PTY parse work per poll tick so a large output burst cannot monopolize the
actor indefinitely:

```rust
const READ_CHUNK: usize = 16 * 1024;
const MAX_READ_PER_TICK: usize = 256 * 1024;
```

Read and parse until `WouldBlock`, EOF, or the budget is exhausted. Publish at
most one snapshot per parse batch, subject to the DEC 2026 gate.

### VT-originated responses

VT-originated responses such as DA reports and cursor reports are VT Actor-local
writes back to the PTY:

```rust
let (response_tx, response_rx) = std::sync::mpsc::channel::<bytes::Bytes>();
vt.on_pty_write(move |_, data| {
    let _ = response_tx.send(bytes::Bytes::copy_from_slice(data));
})?;
```

After each `vt.vt_write(...)`, drain `response_rx` and enqueue those bytes into
`PendingWrites`.

Do not use `Arc<Mutex<Vec<u8>>>` for actor responses.

---

## DEC 2026 synchronized output

Honor DEC 2026 in the VT Actor using libghostty's `Mode::SYNC_OUTPUT` state.

Publication rules:

- If sync output is active after a parse batch, suppress snapshot publication.
- If sync output exits, publish once.
- If the watchdog expires, publish despite active sync output.
- Resize publishes immediately; libghostty resize disables sync output by spec.

The watchdog timeout is 150 ms. Store `sync_deadline: Option<Instant>` in the IO
actor only. Do not expose sync state to the UI, and do not mutate libghostty's
mode directly unless a safe API exists.

---

## Cursor shape and theme

### Cursor shape

Keep the current behavior of seeding the configured default cursor shape into VT
with DECSCUSR bytes, but do it in VT Core on the VT Actor:

- `VtSessionOptions { initial_cursor_shape: CursorShape }`
- feed DECSCUSR during VT Core creation
- publish a snapshot afterward
- hot reload sends `VtCommand::SetCursorShape(shape)`

### Theme colors

Send theme colors to libghostty through VT Core so OSC queries and default
palette behavior remain correct. Snapshot cells still store symbolic colors;
renderer-side theme reload can repaint from the same snapshot.

---

## App integration

The app-side OS-window bundle is `SurfaceState`. The name `Window` is reserved
for the future mux domain (`Window -> Tab -> SplitTree -> Pane`).

`SurfaceState` contains a `seance_mux_client::Pane` even while v1 has one pane
per surface. The pane owns the local VT handle internally, the latest
materialized snapshot, pane-update history, and selection/view state.

### Reads vs commands

UI reads only through pane-view accessors:

- modes for keyboard and mouse encoding
- selection text for copy
- frame source for rendering

UI mutates the terminal only through `seance_mux_client::Pane` commands:

- write
- resize
- scroll lines
- set theme colors
- set cursor shape
- acknowledge presented VT Snapshot generations

### Resize flow

The UI/render side computes grid dimensions because the renderer owns font
metrics, scale factor, and padding.

1. winit resize, scale, font, or padding changes
2. UI updates renderer surface/metrics/padding
3. UI computes `(cols, rows)` through `renderer.grid_size()`
4. UI sends `Pane::resize`
5. Local mux forwards the VT resize command
6. VT Actor resizes VT Core and PTY, then publishes a snapshot
7. UI redraws on pane-scoped frame dirty

On `WindowEvent::Resized`, do not force an immediate draw with a stale snapshot.
Wait for the VT Actor's resized snapshot.

### Render acknowledgement

`App::draw()` records the generation of the pane view's latest materialized VT
Snapshot, renders through `SnapshotFrameSource`, and sends
`ack_presented(generation)` only when `TerminalRenderer::render()` returns
`true`. It does not acknowledge while the surface is occluded and repeated
acknowledgements of the same generation are safe.

`ContentDirty` wake dedupe only controls event-loop wakeups. It is not a dirty
acknowledgement mechanism.

### Mouse wheel and paste

Mouse wheel:

- if latest snapshot modes say mouse tracking is active, encode wheel input and
  send `Write(Bytes)`
- otherwise send `ScrollLines(delta)`

Paste:

- read `snapshot.modes.bracketed_paste`
- prefer one combined `Bytes` payload containing delimiters and paste data to
  preserve ordering and reduce messages

---

## Dependency choice

Use `bytes::Bytes` for write payloads.

- Public `seance-vt` actor API uses `Bytes`.
- Input encoder `Vec<u8>` results become `Bytes::from(vec)`.
- Static escape sequences can use `Bytes::from_static(...)`.
- Composed paste can use `bytes::BytesMut` and `freeze()`.

---

## Why not the rejected alternatives

### Why not make `Terminal` `Send`

Upstream `libghostty-rs` maintainers rejected making `Terminal` `Send` and
recommended an owning thread plus channels. séance follows that boundary. A
local fork or unsafe newtype would make the codebase depend on a property the
upstream library does not promise.

### Why not `Arc<Mutex<Terminal>>`

A mutex around live libghostty state would still allow the UI to borrow or hold
references into libghostty and would require proving lock boundaries around
foreign-owned iterator state. Owned snapshots are simpler: all FFI-backed state
stays actor-local, and the UI receives regular Rust data.

### Why not a renderer thread yet

wgpu surface presentation remains on the winit thread for v1. After parsing
moves to the actor, UI work is bounded by snapshot rendering and GPU submission,
not by shell output volume.

Revisit a renderer thread when median `render()` exceeds 4 ms over a 1000-frame
window with a 4-pane mux active, measured by the frame-time harness planned in
[#26]. If the bottleneck is glyph shaping rather than GPU submission, prefer a
background shaping task over moving the surface itself.

---

## Reference survey

All surveyed terminals keep VT parsing off the windowing thread, but their exact
state-sharing choices differ.

| Aspect               | Ghostty                                | Alacritty                   | WezTerm                   | séance v1 target                      |
| -------------------- | -------------------------------------- | --------------------------- | ------------------------- | ------------------------------------- |
| Threads per terminal | 4 (UI / renderer / IO-write / read)    | 2 (UI / reader+parser)      | 3+ (UI / reader / parser) | 2 (UI / VT Actor)                     |
| Dedicated renderer   | yes                                    | no                          | no                        | no                                    |
| VT parse location    | reader thread                          | reader thread               | parser thread             | VT Actor                              |
| Live VT shared w/UI  | renderer-state mutex + copied snapshot | `FairMutex` around terminal | `Mutex` around terminal   | no; owned `VtSnapshot` only           |
| Wake mechanism       | `xev.Async`                            | `EventLoopProxy` equivalent | mux notification fan-out  | `MuxEvent::Wake`                      |
| Write path           | mailbox to writer thread               | mpsc to IO thread           | locked master writer      | `VtCommand::Write(Bytes)` to VT Actor |

Reference source locations from the previous survey remain useful for context:

- Ghostty — `src/Surface.zig`, `src/termio/Thread.zig`, `src/termio/Exec.zig`,
  `src/renderer/Thread.zig`, `src/renderer/State.zig`,
  `src/renderer/generic.zig`.
- Alacritty — `alacritty_terminal/src/event_loop.rs`,
  `alacritty/src/display/mod.rs`, `alacritty/src/scheduler.rs`.
- WezTerm — `mux/src/lib.rs`, `mux/src/localpane.rs`.

---

## Implementation chain

The actor model + owned snapshot has shipped. The follow-on Phase-1 work that
future remote transports ([#221]) attach to is tracked in [#222]: codec-friendly
types, a `WireFrame` envelope with a `Partial(dirty rows)` variant, and a
server-keyed kitty image cache.

Landing order:

| Stage | Scope                    | Status |
| ----- | ------------------------ | ------ |
| 1     | docs/issues correction   | done   |
| 2     | snapshot model           | done   |
| 3     | VT Actor                 | done   |
| 4     | app integration + rename | done   |

Open follow-ups:

- [#222] in-process client/server seam (Phase 1 wire protocol).
- [#45] `Domain` trait + `LocalDomain` for multi-pane.
- [#171] optional coalesce delay between actor drain and publish.
- [#172] threading stress harness.

Already in code from earlier work:

- [#23] DEC 2026 implemented inside the VT Actor's publication gate.
- [#24] deadline-scheduled redraw shipped UI-side.

Other M2 threading-chain issues (#165-#170) are closed; their work either
shipped under the actor model or was superseded by it.

---

## Do not do

- Do not make or assert `libghostty_vt::Terminal: Send`.
- Do not depend on a forked `libghostty-rs` for thread-safety.
- Do not share libghostty state behind a mutex.
- Do not implement full Domain/tabs/splits/multi-client transport in v1.
- Do not add bell/title/clipboard event expansion in this migration.
- Do not render a stale snapshot immediately after resize unless a later
  measured requirement proves it necessary.

---

## Related docs

- [`docs/architecture.md`](./architecture.md) — pipeline overview and crate map.
