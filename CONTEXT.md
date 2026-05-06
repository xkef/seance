# séance Context

This context names séance's terminal-domain concepts so architecture reviews can
refer to stable project language instead of crate or type names.

## Language

**VT Snapshot**: An immutable séance-owned capture of terminal grid, modes,
cursor, dirty rows, and Kitty graphics for rendering and copy operations.
_Avoid_: live terminal snapshot, render state snapshot

**VT Core**: The live libghostty terminal state plus snapshot extraction logic
shared by the VT Actor and Headless VT. _Avoid_: terminal wrapper, live frame
source

**VT Actor**: The owning thread for one VT Core and its PTY I/O. _Avoid_: worker
thread, reader thread, terminal service

**Pane Session**: Historical name for the pre-mux UI-owned state that combined
one pane's VT command handle, latest VT Snapshot, and selection. New code should
use **Pane View** for client-side pane state. _Avoid_: window session, terminal
state

**Mux Protocol**: The transport-neutral message schema for client/server pane
routing, handshake, ordered pane updates, frame deltas, image cache events, and
classified lifecycle errors. _Avoid_: VT command wire ABI

**Pane Update**: One ordered server update for a pane. It carries a `PaneRef`,
`ServerSeq`, zero or more image cache events, and an optional frame delta.
_Avoid_: VT event, dirty wake

**Frame Delta**: A full snapshot or a base-explicit partial frame that can be
materialized into a VT Snapshot. _Avoid_: latest snapshot diff without base

**Pane View**: Client-side pane state that materializes pane updates and owns
selection/view state. _Avoid_: shared terminal state

**Image Cache Event**: Out-of-band image payload or eviction message ordered
with pane frames. _Avoid_: renderer eviction, placement

**Headless VT**: A PTY-less VT adapter used by tests to feed bytes and produce
VT Snapshots. _Avoid_: fake terminal, direct frame source

**Kitty Graphics Extraction**: The VT Core-internal capture of Kitty graphics
placements, image payloads, virtual placeholders, and placeholder-cell text
suppression for inclusion in a VT Snapshot. _Avoid_: image renderer, graphics
adapter

## Relationships

- A **VT Actor** owns exactly one **VT Core**.
- A **Headless VT** owns exactly one **VT Core**.
- A **VT Core** produces zero or more **VT Snapshots**.
- A **VT Actor** publishes zero or more **VT Snapshots**.
- A **Pane View** materializes **Pane Updates** into a current **VT Snapshot**.
- A **Pane View** sends commands through the mux to exactly one local or remote
  pane owner.
- A **Pane Update** may carry a **Frame Delta** and **Image Cache Events**.
- A **Headless VT** uses the same extraction path as a **VT Actor** to produce
  **VT Snapshots**.
- **Kitty Graphics Extraction** is part of **VT Core**'s **VT Snapshot**
  production path.

## Example dialogue

> **Dev:** "Should the Layer 4 dump walk the headless terminal directly?"
> **Domain expert:** "No. A **Headless VT** should produce a **VT Snapshot**,
> and the dump should read that snapshot the same way the renderer does."

## Flagged ambiguities

- "snapshot" can mean libghostty's render snapshot or séance's **VT Snapshot**.
  Use **VT Snapshot** only for the séance-owned immutable data; say "libghostty
  render snapshot" for the FFI value.
- "terminal" can mean the app, the live libghostty terminal, or the user's
  shell. Use **VT Core**, **VT Actor**, **Pane Session**, or **VT Snapshot**
  when those are the intended concepts.
