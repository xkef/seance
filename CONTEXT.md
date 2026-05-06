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

**Pane Session**: The UI-owned state for one pane's VT command handle, latest VT
Snapshot, and selection. _Avoid_: window session, terminal state

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
- A **Pane Session** keeps the latest **VT Snapshot** for rendering and copy.
- A **Pane Session** sends commands to exactly one **VT Actor**.
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
