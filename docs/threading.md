# Threading

This document is the architectural decision record for séance's threading model.
It supersedes the diagnostic preamble in [#165] and is the contract the rest of
[M2] is built against. Sub-issues that carry the work are linked inline; see the
implementation chain at the bottom for the full grid.

[#165]: https://github.com/xkef/seance/issues/165
[m2]: https://github.com/xkef/seance/issues/5

## Status

| Concern                | Today (`main`)                                     | v1 target (this doc)                                       |
| ---------------------- | -------------------------------------------------- | ---------------------------------------------------------- |
| PTY blocking `read`    | `seance-pty-reader` thread, reads only             | renamed `seance-io`; same thread reads, parses, writes     |
| VT parser (`vt_write`) | UI thread, inside `App::user_event(PtyData)`       | IO thread, under `Arc<FairMutex<VtState>>`                 |
| Cell rebuild + GPU     | UI thread, holds `&Terminal` for the entire frame  | UI thread, under a snapshot taken in a short locked window |
| Keystroke → PTY write  | UI thread → `Terminal::write` (`writer.write_all`) | UI thread → mpsc → IO thread `writer.write_all`            |
| Wake from IO           | per-chunk `EventLoopProxy::send_event(PtyData)`    | one `ContentDirty` per drained burst (AtomicBool dedupe)   |
| DEC 2026 watchdog      | not implemented                                    | armed on IO thread, 150 ms                                 |
| Renderer thread        | none                                               | none — revisit metric below                                |

`seance-pty-reader` is the only off-UI thread today
(`crates/seance-app/src/io.rs:20`). It funnels every read chunk back into the
winit event loop via a `UserEvent::PtyData(Vec<u8>)`, where the parser runs on
the UI thread inside `App::user_event` (`crates/seance-app/src/app.rs:228`).
That makes input/resize jank during heavy shell output a real possibility, which
is the gap this doc closes.

---

## Decision

**Adopt Alacritty's two-thread shape for v1.**

```
┌─ UI thread (winit) ────────────────────┐   ┌─ IO thread ──────────────────┐
│ input, config reload, resize, redraw   │   │ owns VT Terminal + PTY       │
│ owns wgpu renderer + glyph atlas       │   │ owns master fd reader/writer │
│ snapshots VtState under FairMutex      │   │ blocking poll + parse        │
│ owns selection state                   │   │ mutates VtState under same   │
│                                        │   │   FairMutex                  │
│ EventLoopProxy ◀── ContentDirty ───────│   │ AppToIo mailbox ◀── UI       │
│ AppToIo mailbox sender ────────────────│──▶│ EventLoopProxy ─▶ UI         │
│                                        │   │ DEC 2026 watchdog (150 ms)   │
└────────────────────────────────────────┘   └──────────────────────────────┘
```

The mutex is shared (`Arc<FairMutex<VtState>>`); the UI takes it briefly each
frame to snapshot, the IO thread takes it for every parse burst. The PTY fd
reader and writer are owned exclusively by the IO thread — the UI never touches
them, even via the mutex.

The decision is unambiguous on three points:

1. **One IO thread, not three.** PTY reads, VT parsing, and PTY writes all live
   on the same thread, gated by a single `parking_lot::FairMutex`. No separate
   "writer thread" or "renderer thread" until profiling demands one (criterion
   below).
2. **Shared state lives behind a mutex, not a channel of deltas.** The UI
   snapshots cells under the lock and rebuilds outside it (Ghostty's `Critical`
   pattern). We do not stream `VtAction` deltas.
3. **`std::sync::mpsc` + `winit::EventLoopProxy` are sufficient.** No `libxev`,
   no custom event loop, no `tokio`.

Everything else in this doc justifies those three calls.

---

## Why not the alternatives

### Why not Ghostty's four-thread shape (UI / renderer / IO-writer / IO-reader)

Ghostty splits the renderer onto its own thread because the UI thread runs
AppKit + Metal frame submission concurrently with input handling, and it runs a
dedicated writer thread because every `vt_write` from output bursts holds a big
`renderer_state.mutex` that a same-thread writer would contend with. Neither
pressure exists for séance v1: wgpu's `Surface::present` is cheap on the winit
thread, and we have no measured contention because we do not have an IO thread
yet. Adopting that shape now means writing four mailboxes, four shutdown paths,
and four lifetime stories without a profile to point at.

The model can grow toward Ghostty's shape in two steps if needed (renderer
thread first; writer thread second), and the mailbox protocol in [#168] is
designed to accommodate that without a rewrite.

### Why not a single foreground thread with `VtAction` deltas (GPUI-shape)

GPUI in `xkef/zed` runs all UI-side state on one foreground thread; the
canonical pattern is "parse on a background `Task<T>`, ship deltas to the
foreground, mutate there, render lock-free." That eliminates the mutex entirely
— no `FairMutex`, no snapshot copy, no lock-hold budget.

Ruled out for v1 because:

- **Heavy bursts arrive on the UI tick.** A 100 MiB `cat` produces several MiB
  of `VtAction`s; applying them between frames either drops frames or
  backpressures the IO thread. Ghostty's mutex-plus-snapshot model degrades more
  gracefully under shell flooding than a delta stream does, because the UI can
  choose when to pay the snapshot cost.
- **The selection / scrollback model wants random read access to grid state.**
  Mouse drag, cursor reports, and OSC responses all read arbitrary cell ranges.
  With deltas we would still need a copy of the grid on the UI side; we would
  have just moved the mutex to the delta-apply path.
- **`libghostty-vt` is the system of record for grid state.** Replacing it with
  a delta-applier on the UI side either duplicates that state or marshals every
  read back to the IO thread. Both lose more than they buy.

The GPUI shape stays attractive when séance later grows agent / mux / modal
state that genuinely needs single-thread ownership; that state can sit on the UI
thread without disturbing the VT mutex underneath. See [M6][m6-link] and the
comment thread on [#165].

[m6-link]: https://github.com/xkef/seance/issues/9

### Why not WezTerm's two-thread-per-pane shape (reader + parser)

WezTerm runs reader and parser on separate threads with a socketpair between
them so the reader cannot block the parser. With one terminal per window, séance
v1 has no reader to back up; the IO thread parses while it reads, and the OS
kernel buffers PTY output between reads. We inherit WezTerm's coalesce-delay
idea ([#171]) without the second thread.

---

## Reference survey

### Ghostty (libxev + SPSC mailboxes, per-surface)

Four threads per surface:

- **UI** (apprt main) — `src/Surface.zig:585-733`
- **Renderer** — `src/renderer/Thread.zig`
- **IO writer** — `src/termio/Thread.zig`
- **PTY reader** — `src/termio/Exec.zig:1256-1403` (`ReadThread`, named
  `"io-reader"`)

Reader sets master fd `O_NONBLOCK`, tight-loops `read` until `EWOULDBLOCK`, then
`posix.poll(&[pty_fd, quit_pipe], -1)`. The VT parse runs **on the reader
thread** under `renderer_state.mutex` (`src/renderer/State.zig:10-18`). Cell
rebuilding happens **outside** the lock using a copied `Critical` snapshot
(`src/renderer/generic.zig:1156-1260`). Wake is an `xev.Async` per thread;
mailboxes are `BlockingQueue<Message, 64>` with an explicit "drop mutex, notify,
reacquire" dance when a queue fills (`src/termio/mailbox.zig:61-95`). DEC 2026
watchdog is **1 s**, armed on the IO thread's xev loop; renderer early-returns
when `terminal.modes.get(.synchronized_output)`.

The dedicated writer thread exists specifically to offload PTY-write, resize,
and config from the hot parse path (`src/termio/Thread.zig:1-9`);
`linefeed_mode` is duplicated as an atomic-packed flag so writes don't need the
big mutex.

### Alacritty (one IO thread, `FairMutex`)

`alacritty_terminal/src/event_loop.rs:217` spawns a single **PTY+parser
thread**. Blocking read via the `polling` crate;
`ansi::Processor::advance(&mut term, &buf)` runs on that thread (`:169`). `Term`
is `Arc<FairMutex<Term<U>>>` (`:49`). The reader uses `try_lock_unfair()` with a
fallback to a blocking lock depending on buffer size (`:140-145`). The UI
(winit) is woken via `event_proxy.send_event(Event::Wakeup)` (`:180`). Commands
from UI → IO flow via `mpsc` drained in `drain_recv_channel()` (`:97`). No
dedicated renderer thread.

The UI takes the mutex once per frame, builds renderer state, drops the mutex
before invoking wgpu (`alacritty/src/display/mod.rs:775-815`).

### WezTerm (two threads per pane: reader + parser, pipe-buffered)

`mux/src/lib.rs` `add_pane` spawns `read_from_pane_pty(pane, banner, reader)`
which in turn spawns a second thread running
`parse_buffered_data(pane, dead, rx)`. A **socket pair** sits between them so
the reader can't block the parser. `termwiz` parses off into
`termwiz::escape::Action`s that are flushed to the mux in coalesced "frames" to
reduce notifications (`mux/src/lib.rs:199-226`,
`mux_output_parser_coalesce_delay_ms`). The mux notifies subscribers via
`mux.notify(MuxNotification::PaneOutput(pane_id))`. `Terminal` is
`Mutex`-guarded (`mux/src/localpane.rs`).

### Synthesis

| Aspect                | Ghostty                                | Alacritty                    | WezTerm                              |
| --------------------- | -------------------------------------- | ---------------------------- | ------------------------------------ |
| Threads per terminal  | 4 (UI / renderer / IO-write / read)    | 2 (UI / reader+parser)       | 3+ (UI / reader / parser)            |
| Dedicated renderer    | yes                                    | no                           | no                                   |
| VT parse location     | reader thread, big mutex               | reader thread, `FairMutex`   | parser thread, `Mutex`               |
| Wake mechanism        | `xev.Async`                            | `EventLoopProxy::send_event` | `MuxNotification` fan-out            |
| Write path            | UI → mailbox → `xev.Stream.queueWrite` | UI → mpsc → IO thread        | UI → locked Mutex → `master.write()` |
| Cell-build under lock | no — copies `Critical` snapshot        | yes (brief)                  | partial                              |

What they all share: **the VT parser does not run on the windowing thread.**

---

## Architecture, in detail

### Shared state

```rust
// crates/seance-vt/src/shared.rs (planned, #167)
pub struct VtState {
    vt: Box<libghostty_vt::Terminal<'static, 'static>>,
    render_state: libghostty_vt::RenderState<'static>,
    response_buf: Vec<u8>,      // VT → PTY responses (DA1, cursor reports, …)
    bell_count: u32,
    exit_status: Option<ExitStatus>,
    // Per-cell pixel dimensions, propagated from resize. Read-only
    // outside the resize path.
    cell_width_px: u32,
    cell_height_px: u32,
}

pub type SharedVt = Arc<parking_lot::FairMutex<VtState>>;
```

What is **not** in `VtState`:

- **Selection.** Hoisted to UI-side state ([#166]). Mouse drag / clipboard
  updates do not need to round-trip through the IO thread.
- **Master PTY fd, reader, writer.** Owned exclusively by the IO thread.
  Crossing the mutex boundary for a `write_all` would defeat the purpose of
  having a separate IO thread.
- **Image cache state.** Renderer-side (already the case).

### Lock discipline

- **IO thread** locks for every `vt_write` burst, capped at
  `MAX_LOCKED_READ = 64 KiB` per acquisition so a 100 MiB `cat` cannot starve
  the renderer. After the cap, drop the lock, return to the poller, and pick up
  the next chunk on the next iteration.
- **UI thread** locks once per frame to take a `VtSnapshot` ([#170]). The
  snapshot copies cells (≤ 640 KiB at 200×50, ≤ 50 µs at main-memory bandwidth),
  reads cursor / modes / dirty rows, then drops the lock. Cell rebuild, glyph
  shaping, atlas upload, and GPU encoding all run unlocked.
- **Resize** is UI-initiated. UI sends `AppToIo::Resize` via the replace-latest
  slot; IO acquires the lock, calls `vt.resize` + `master.resize`, drops the
  lock, signals `ContentDirty`. Resize is the only path that mutates
  `cell_*_px`, and it happens under the same lock UI snapshots take.

Lock-hold budget: **median < 500 µs / p99 < 2 ms** at 200×50, measured via
`tracing::info_span!("render_snapshot")`. If we exceed the p99 budget, fall back
to WezTerm-style seqno-per-row delta snapshots (`mux/src/localpane.rs:182-192`)
— that is, only copy rows newer than the last frame's seqno. The dirty-row work
in [#20][issue-20] is the foundation.

[issue-20]: https://github.com/xkef/seance/issues/20

### Mailbox protocol

The UI → IO direction is **not** a single `mpsc`. Different message classes have
different backpressure semantics:

| Message          | Channel            | Backpressure         | Why                                                    |
| ---------------- | ------------------ | -------------------- | ------------------------------------------------------ |
| `Write(Bytes)`   | unbounded mpsc     | block (or large cap) | dropping keystrokes is unacceptable                    |
| `Resize`         | `Mutex<Option<_>>` | replace-latest       | live-resize fires many events; only the latest matters |
| `SetThemeColors` | `Mutex<Option<_>>` | replace-latest       | hot-reload triggers one event                          |
| `ScrollLines`    | `AtomicI32`        | accumulate           | wheel bursts should sum, not queue                     |
| `ClearScreen`    | mpsc<Control>      | queue                | explicit user action                                   |
| `Shutdown`       | mpsc<Control>      | queue                | terminal                                               |

Every mutator on the UI side pokes a **wake pipe** registered with the poller,
so the IO thread's `poll::wait` returns immediately rather than sleeping until
its next deadline. This is Alacritty's `Poller::notify()` pattern
(`alacritty_terminal/src/event_loop.rs:392`).

The IO → UI direction goes through `winit::event_loop::EventLoopProxy` as new
variants of the existing `UserEvent` enum (`crates/seance-app/src/main.rs:25`):

| Variant                   | Coalesced?            | Notes                                       |
| ------------------------- | --------------------- | ------------------------------------------- |
| `ContentDirty`            | yes (AtomicBool gate) | exactly one in flight at a time             |
| `BellRing { count }`      | no                    | each is distinct UI work                    |
| `OscResponse(Bytes)`      | no                    | DA1, cursor report, OSC 11 reply, …         |
| `ChildExited(ExitStatus)` | no                    | one per terminal lifetime                   |
| `ClipboardRequest { … }`  | no                    | OSC 52, `oneshot::Sender<Bytes>` round-trip |

`PtyData(Vec<u8>)` is removed — its replacement is `ContentDirty` plus the
locked snapshot. The reader thread is replaced by the IO thread's poller, so
`crates/seance-app/src/io.rs` goes away.

### Wake protocol (IO → UI)

```rust
pub struct UiWaker {
    proxy: EventLoopProxy<UserEvent>,
    pending: Arc<AtomicBool>,
}

impl UiWaker {
    pub fn wake_content_dirty(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            let _ = self.proxy.send_event(UserEvent::ContentDirty);
        }
    }
}

// In App::user_event(UserEvent::ContentDirty):
self.waker_pending.store(false, Ordering::Release);
self.mark_dirty();
```

`AcqRel` on the swap ensures the IO thread's `vt_write` happens-before the UI's
mutex acquisition in `user_event`. The mutex itself synchronizes (so `Acquire`
would suffice), but `AcqRel` is the safer default. The pending flag is cleared
**before** snapshotting; otherwise the IO thread could dirty the grid after
snapshot but before clear, and we'd miss a wake.

### DEC 2026 (synchronized output) flow

DEC 2026 is the explicit handshake by which a TUI tells the terminal "don't
redraw mid-frame": `\x1b[?2026h` to begin, `\x1b[?2026l` to commit. This
protocol is what lets tmux and modern TUIs avoid tearing. See [#23] for the full
integration.

```
IO thread:                                     UI thread:
┌──────────────────────────────────────┐
│ vt_write(... CSI?2026h ...)          │
│   sync_active = true                 │
│   sync_deadline = now + 150 ms       │   (no wake — UI holds last frame)
│ vt_write(... cells ...)              │
│   sync_active still true, no wake    │
│ vt_write(... CSI?2026l ...)          │
│   sync_active = false                │
│   waker.wake_content_dirty()         │──▶ ContentDirty → snapshot → render
└──────────────────────────────────────┘
```

If the watchdog fires (now ≥ sync_deadline) before the closing sequence, IO
forces sync_active off and signals ContentDirty. 150 ms matches xterm; Ghostty's
1 s is too lenient for our latency target.

The deadline is held in IO-local state (no atomic / no lock) because nothing
else reads it. The UI does **not** need to know whether sync is active — it just
doesn't get woken until the IO side decides.

### Deadline-scheduled redraw (already shipped)

[#24] is closed; `cf4a1b1` replaced the 4 ms PTY poll with
`ControlFlow::WaitUntil(next_animation_deadline)`. The deadline scheduler
remains UI-side and is orthogonal to threading: it controls **when** the next
redraw fires given outstanding animation deadlines (cursor blink, SGR blink,
bell, Kitty GIF frames, custom shaders). Content-dirty wakes are punctual via
the proxy; animation wakes are scheduled via `WaitUntil`. Both paths feed
`mark_dirty`.

The post-#24 form of `about_to_wait` does not care about the threading refactor
— it already treats PTY wakes as out-of-band events.

### Shutdown ordering

```
1. UI receives WindowEvent::CloseRequested.
2. UI sets a shared AtomicBool kill-switch and sends AppToIo::Shutdown via
   the control channel + pokes the wake pipe.
3. UI joins the IO thread with a 500 ms timeout. The IO loop checks the
   kill-switch on every poll iteration, so even a wedged mailbox cannot
   prevent shutdown.
   - Within timeout: IO drains its mailbox, drops the writer (closes the
     master fd), reaps the child via try_wait, exits cleanly.
   - Timeout: UI force-closes by dropping its mailbox sender; the next
     IO loop iteration sees the kill-switch and exits, leaking the PTY
     fd to the kernel cleanup on process exit.
4. UI drops the renderer (wgpu surface destroyed last).
```

Child-exit ordering: the IO thread observes `read() == 0` (EOF) and sends
`IoToApp::ChildExited(status)` via `try_wait`. The UI handles exit on the next
event-loop iteration; if the user has already closed the window, the event is
dropped silently.

### Send/Sync audit (current blocker, [#166])

`seance-vt::Terminal` is `!Send` today
(`crates/seance-vt/src/terminal.rs:78,82`):

- `response_buf: Rc<RefCell<Vec<u8>>>` — `Rc` is `!Send`. Captured by the
  `vt.on_pty_write` callback.
- `writer: RefCell<Box<dyn Write + Send>>` — `RefCell` is `!Sync`; fine for
  single-owner move, but only if we drop the `&self`-takes-lock pattern.

The PNG decoder install (`terminal.rs:62-67`) is thread-local inside
libghostty-vt, so whichever thread owns the VT must be the one to call
`set_png_decoder`. Today that's the UI thread; under v1 it must move to the IO
thread spawn callsite.

[#166] is the prerequisite: rewrite `Terminal` so an `assert_send::<Terminal>()`
test compile-passes, and move the PNG decoder install to a per-thread callable.
The upstream `libghostty-vt::Terminal` `Send` story also gets audited there — if
`Uzaaft/libghostty-rs` lacks `unsafe impl Send`, we propose a PR.

---

## Why not a renderer thread (yet)

We do not move wgpu's `Surface::configure` / `Queue::write_buffer` /
`Surface::present` off the winit thread for v1. The reasoning:

1. winit on macOS expects the surface to be touched from the main thread. Moving
   submission to a worker thread requires either double-buffered surface
   ownership (Ghostty's pattern) or trusting `wgpu::Surface: Send + Sync`, which
   is platform-dependent.
2. We have no measured contention. Today the UI does PTY parse + render in the
   same frame; once parse moves to the IO thread, the UI is doing snapshot +
   cell rebuild + GPU encode + present — all bounded by VT grid size (200×50 ≈
   10 000 cells), not by shell throughput.
3. The shape composes: if profiling later shows the renderer is the bottleneck,
   lifting cell rebuild + GPU encode onto a worker is a self-contained change
   that doesn't disturb the IO thread or the mutex protocol.

**Revisit metric:** add a renderer thread when **median `render()` > 4 ms over a
1000-frame window with 4-pane mux active**, measured via the `seance-bench`
frame-time harness ([#26][issue-26]). Below that threshold the cost of a
thread-cross + double-buffered surface exceeds the win. The Mux work in
[M6][m6-link] is the most likely trigger; a single-pane idle terminal will not
approach 4 ms.

If we cross the threshold and the hot work is glyph shaping or atlas upload (not
GPU encode), the right move is a **background `Task<T>` for shaping**, not a
dedicated renderer thread. GPUI's pattern is the template: shape / measure /
layout off-thread, ship the result back via a channel, render on the foreground.

[issue-26]: https://github.com/xkef/seance/issues/26

---

## Open questions resolved

| #   | Question                                        | Resolution                                                                     |
| --- | ----------------------------------------------- | ------------------------------------------------------------------------------ |
| 1   | `Mutex` vs `RwLock` vs `parking_lot::FairMutex` | `parking_lot::FairMutex` — Alacritty pattern, avoids reader starvation         |
| 2   | Bounded vs unbounded `mpsc`                     | `Write` unbounded (or 4096-cap); coalesceable types use slots                  |
| 3   | Is `libghostty-vt::Terminal` `Send`?            | audit deferred to [#166]; PR upstream if necessary                             |
| 4   | PNG decoder install location                    | move to IO-thread spawn (`install_png_decoder_for_this_thread()`)              |
| 5   | Selection state ownership                       | hoist to UI side; `selection_text(&Selection)` reads under lock                |
| 6   | `crossbeam::channel` or `std::sync::mpsc`?      | `std::sync::mpsc`; we don't use `select!` and dependency surface stays smaller |
| 7   | `Task<T>` cancel-on-drop primitive              | not adopted v1; revisit when agent / mux state lands ([#9][m6-link])           |

---

## Implementation chain

[#165] (this doc) is the parent. The work splits into six sub-issues that land
in order; [#23] and [#171] are orthogonal but listed for context.

| Issue  | Title                                                                | Depends on       |
| ------ | -------------------------------------------------------------------- | ---------------- |
| [#166] | make `seance-vt::Terminal` `Send`; PNG decoder per-thread            | #165             |
| [#167] | spawn dedicated IO thread (reader + parser + writer)                 | #165, #166       |
| [#168] | mailbox protocol — `AppToIo` / `IoToApp` enums, replace-latest slots | #167             |
| [#169] | dedup IO → UI wakes via `AtomicBool` + `EventLoopProxy`              | #167, #168       |
| [#170] | snapshot VT under lock, build cells outside (`Critical` pattern)     | #167             |
| [#171] | optional WezTerm-style coalesce delay (config'd, default 2 ms)       | #167             |
| [#172] | bench: threading stress tests                                        | #167, #169, #170 |
| [#23]  | DEC 2026 synchronized output                                         | #167             |
| [#24]  | deadline-scheduled redraw — **closed**, `cf4a1b1`                    | —                |

[#23]: https://github.com/xkef/seance/issues/23
[#24]: https://github.com/xkef/seance/issues/24
[#166]: https://github.com/xkef/seance/issues/166
[#167]: https://github.com/xkef/seance/issues/167
[#168]: https://github.com/xkef/seance/issues/168
[#169]: https://github.com/xkef/seance/issues/169
[#170]: https://github.com/xkef/seance/issues/170
[#171]: https://github.com/xkef/seance/issues/171
[#172]: https://github.com/xkef/seance/issues/172

---

## References

Reference-terminal source citations:

- Ghostty — `src/Surface.zig:585-733`, `src/termio/Thread.zig`,
  `src/termio/Exec.zig:1256-1403` (`ReadThread`),
  `src/termio/Termio.zig:463-701`, `src/renderer/Thread.zig`,
  `src/renderer/State.zig:10-18`, `src/renderer/generic.zig:1156-1260`
  (`Critical` snapshot pattern), `src/termio/mailbox.zig:61-95`
  (deadlock-avoidance dance).
- Alacritty — `alacritty_terminal/src/event_loop.rs:49,97,125-180,217-298`
  (`FairMutex`, `pty_read`, `Notifier`, spawn);
  `alacritty/src/display/mod.rs:775-815` (drop the mutex before wgpu);
  `alacritty/src/scheduler.rs:32` (input-priority frame topic).
- WezTerm — `mux/src/lib.rs` (`read_from_pane_pty` + `parse_buffered_data`,
  coalesce delay at `:199-226`), `mux/src/localpane.rs` (`self.terminal.lock()`,
  `emit_output_for_pane`; seqno snapshots at `:182-192`).

Internal:

- [`docs/architecture.md`](./architecture.md) — pipeline overview, "Event loop &
  redraw" section, component choices.
