# Client/server seam plan

Goal: implement #222 as one focused Phase-1 PR that creates a remote-ready
protocol seam while still shipping only a local in-process mux adapter and
preserving current single-pane behavior.

Do not serialize `seance-vt`'s actor API as the wire ABI. `VtCommand`,
`VtEvent`, `VtSessionHandle`, and live `libghostty-vt` state remain local
implementation details. The protocol is mux-level: identity, handshake,
request/reply correlation, ordered pane updates, base-explicit frame deltas,
image cache events, replay/resync, flow-control limits, and classified lifecycle
errors.

## Architecture split

The server side is a headless terminal kernel:

- PTYs and subprocess lifecycle
- VT parser and scrollback
- shared session/domain/window/tab/pane topology
- shared image registry and pane frame history

The client side owns human-facing state:

- fonts, colors, themes, key bindings, status bar, GPU pipeline
- copy mode, selection view state, command palette, clipboard
- rich UI scripting and per-user/per-machine preferences

Litmus test: if another client attached to the same session should see it
differently, keep it client-side.

## Crate seams

Target dependency direction:

```text
seance-app -> seance-mux-client, seance-render, seance-input, seance-config
seance-mux-client -> seance-vt, seance-protocol, seance-frame
seance-vt -> seance-protocol, seance-frame
seance-render -> seance-protocol, seance-frame
seance-input -> seance-protocol
seance-render-test -> seance-vt, seance-protocol, seance-frame
```

Crate responsibilities:

- `seance-protocol`: owned serializable protocol and frame data only. No
  `winit`, `wgpu`, `libghostty-vt`, `portable-pty`, `seance-render`, or
  `seance-vt` dependency.
- `seance-frame`: `FrameSource`, visitors, `SnapshotFrameSource`, and borrowed
  non-serializable render views.
- `seance-mux-client`: local pane facade, pane-update materialization,
  selection/view state, replay history, and app-facing commands. It owns the
  local `VtSessionHandle` internally.
- `seance-vt`: VT Core, PTY actor, local actor commands/events, and libghostty
  integration.

## Protocol shape

- Cells over the wire, not raw PTY bytes as render state.
- `Hello` carries min/max version, capabilities, max message/image sizes, and
  optional last-seen sequence for compatibility negotiation.
- Length-prefixed postcard framing uses varint length plus compression flag.
- `Envelope { request_id, server_seq, kind, payload }` supports request/reply
  correlation and unilateral server pushes.
- Future transports may map concerns onto `CONTROL`, `INPUT`, `OUTPUT`, and
  `IMAGES` streams to avoid head-of-line blocking.
- Unknown message kinds fail cleanly. Version mismatch, unsupported capability,
  bad route, stale pane, need-full, size limits, corruption, pane exit, EOF,
  detach, and server pane errors are classified.

Protocol identities are opaque newtypes: protocol version, capability, request
ID, server sequence, generation, server/session/client/domain/window/tab/pane
IDs, pane epoch, and image ID. Pane-routed messages carry
`PaneRef { pane_id, epoch }`.

## Frame updates

`FrameDelta` has two variants:

- `Full { generation, snapshot }`
- `Partial { base_generation, generation, cols, rows, cursor, modes, placements, dirty_rows }`

A partial applies only when the client's materialized snapshot generation
matches `base_generation` and dimensions match. Otherwise the client requests or
receives a full reset.

`RowDelta` uses row-local text offsets. Dirty rows must be sorted and unique;
row indices must be in range; cell count must equal `cols`; offsets must be
valid UTF-8 boundaries. Applying a partial rebuilds the materialized text arena
and rewrites every `SnapshotCell.text_start`.

## Replay and pull lines

`PaneUpdate { pane, seq, image_events, frame }` is the ordered update unit.
`seance-mux-client` keeps a per-pane history ring plus latest-full fallback.
First attach sends current topology/full state. Resume with retained
`last_seen_seq` replays updates; missing history sends resync/full reset.

The primary render path is push-based pane updates. The protocol also reserves
`GetLines { range, since_seq }` / `Lines` for bounded scrollback or viewport
pulls so a client does not need the server to push an unbounded scrollback log.

## Image cache

Renderer cache keys are scoped: `ImageKey { pane: PaneRef, image_id: ImageId }`.
Frames carry placement refs. Image puts, chunks, completes, and evicts are out
of band but ordered with pane updates. Server eviction means the server-side
payload is deleted; renderer LRU eviction is local. If a frame references a
locally missing image, the client sends `ImageCacheMiss` and skips the placement
until re-put.

## Flow control

Initial protocol constants cover decoded message bytes, PTY input bytes, pending
input bytes per client, image chunk bytes, pending outbound bytes per client,
and retained pane updates. Frame updates may coalesce to latest, but losing a
required partial base forces a full frame. Topology, pane exit, errors, and
lifecycle messages are not dropped.

## Persistence boundaries

Persistence is not part of #222, but the seam assumes separate lifetimes:

- server in-memory ephemeral: live PTYs, VT state, dirty tracking
- server append-only on disk: optional scrollback with explicit privacy policy
- server transactional on disk: topology metadata for cold restart, not live
  process recovery
- client static config: declarative settings
- client dynamic preferences: per-user/per-machine state
- client cache: recomputable data

Server persists shared truth that survives clients. Client persists
per-user/per-machine state. Nothing persists what can be recomputed.

## Scope

In scope:

- `seance-protocol` and `seance-frame`
- local `seance-mux-client` single-pane adapter
- `seance-app` rewired through `seance-mux-client`
- protocol codec/schema tests
- frame-delta application tests
- mux history/replay tests
- docs updates

Out of scope:

- Unix socket, SSH, TLS, QUIC, daemon discovery, and real multi-client transport
  (#221)
- full Domain/LocalDomain tabs/splits/window UI (#45)
- coalesce-delay (#171)
- threading stress harness (#172)

PR footer:

```text
Closes #222
Refs: #45
Refs: #221
Refs: #5
```
