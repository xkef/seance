# Mux Protocol

The mux protocol is the transport-neutral contract between a séance client view
and a pane-owning server. Phase 1 ships the schema, codec, local in-process mux
adapter, and simulated replay tests. It does not open sockets or implement
SSH/TLS/multi-client transports; those remain under #221.

## Goals

The server is a headless terminal kernel: PTYs, VT parser, scrollback, and
shared session/pane/tab/window topology. The client owns the human-facing
surface: fonts, colors, key bindings, themes, status bar, GPU pipeline,
scripting UI, copy mode, and command palette.

The litmus test for server-side state is: would a different client connected to
the same session expect to see this differently? If yes, it belongs client-side.

- Keep live `libghostty-vt` state inside VT Core and the VT Actor.
- Send parsed cells and metadata over the protocol, not terminal input bytes
  masquerading as render state.
- Route every pane command and update through mux-level messages, not
  `VtCommand`, `VtEvent`, or app/render identifiers.
- Make remote compatibility explicit before transports exist.
- Let attach/reconnect rebuild topology and current pane state without depending
  on a durable event log.
- Keep full frames, base-explicit partial frames, image cache events, and
  lifecycle errors ordered.

## Handshake and capabilities

Clients begin with
`Hello { min_version, max_version, capabilities, max_message_bytes, max_image_bytes, last_seen_seq }`.
Servers answer with
`ServerHello { version, capabilities, server_id, session_id }` or a structured
`VersionMismatch` / `UnsupportedCapability` error.

Phase 1 defines these capabilities:

- `FrameDelta`
- `ImageCache`
- `ImageChunks`
- `Resume`
- `Zstd`

`Zstd` is only negotiated when both sides advertise it. A frame with the
compression bit set before negotiation is a protocol error.

## Envelope, framing, and codec

Every transport carries ordered length-prefixed protocol data units:

```text
varint(length << 1 | compressed_bit) || postcard(Envelope)
```

Future network transports may map protocol concerns onto separate streams
(`CONTROL`, `INPUT`, `OUTPUT`, `IMAGES`) to avoid image transfers head-of-line
blocking typing or pane lifecycle messages. The protocol crate defines a
frame-oriented `Transport` trait, an mpsc-backed `InProcessTransport`, and typed
client/server frame codecs. The production single-binary app path uses a direct
`LocalDomain`; `ProtocolDomain` uses the same Domain seam over `Transport` for
remote transports and conformance tests.

`Envelope` contains:

- `request_id`: non-zero for client request/reply correlation, zero for
  unilateral server pushes.
- `server_seq`: ordered server sequence for pushed updates.
- `kind`: explicit numeric message kind.
- `payload`: postcard-encoded payload for that kind.

Unknown message kind values fail as `UnknownMessage`; they do not panic or
deserialize into an arbitrary enum variant. Decoders enforce
`MAX_DECODED_MESSAGE_BYTES` before payload decode and classify truncated frames,
oversized frames, bad compression flags, and corrupted payloads separately.

## Identity model

Protocol identities are opaque newtypes:

- `ServerId`, `SessionId`, `ClientId`
- `DomainId`, `WindowId`, `TabId`, `PaneId`, `PaneEpoch`
- `ImageId`

Pane-routed messages use `PaneRef { pane_id, epoch }`. The epoch lets a server
reject stale commands if pane IDs are ever recycled. The app and renderer do not
consume future remote IDs directly; the mux client adapter owns the mapping
between remote identities and local pane views.

## Messages

The main render path is push-based: the server sends ordered `PaneUpdate`
notifications. The protocol also reserves pull requests for scrollback/viewport
line content (`GetLines { range, since_seq }` and `Lines`) so a client can ask
for a bounded cell range without forcing the server to push an unbounded
scrollback log.

Client payloads include:

- `Hello`
- `Subscribe`
- `SpawnPane`
- `ClosePane`
- `ResizePane`
- `ScrollPane`
- `SetPaneTheme`
- `SetPaneCursorShape`
- `PaneInput`
- `RequestSnapshot`
- `ImageCacheMiss`
- `AckApplied`
- `AckPresented`
- `Ping`
- `GetLines`

Server payloads include:

- `Hello`
- `Error`
- `Topology`
- `PaneUpdate`
- `PaneExited`
- `ResyncRequired`
- `Pong`
- `Lines`

`VtCommand` and `VtEvent` remain local VT Actor implementation details. The
local Domain maps them into pane-scoped mux wakes and ordered Pane Updates.

## Frame deltas

`FrameDelta` has two forms:

```rust
Full { generation, snapshot }
Partial {
    base_generation,
    generation,
    cols,
    rows,
    cursor,
    modes,
    placements,
    dirty_rows,
}
```

A partial applies only when the client materialized snapshot generation equals
`base_generation` and the dimensions match. Otherwise the client returns
`NeedFull` and requests or receives a full reset.

`RowDelta` contains one viewport row, row-local cells, and a row-local text
arena. Cell offsets must be valid UTF-8 boundaries inside that row arena. Dirty
rows must be sorted and unique. Applying a partial rebuilds the full snapshot
text arena and rewrites every `SnapshotCell.text_start` offset.

Applied dirty state is:

- `Full` -> `DirtySnapshot::Full`
- partial with dirty rows -> `DirtySnapshot::Partial(rows)`
- metadata-only partial -> `DirtySnapshot::Clean`

Metadata-only partials still require redraw scheduling because cursor, modes,
and placements can change even when cell buffers stay clean.

## Pane updates and replay

`PaneUpdate { pane, seq, image_events, frame }` is the ordered server update
unit. Image events in a `PaneUpdate` apply before the frame in the same update.

The pane-owning Domain keeps a per-pane `PaneFrameHistory` ring plus the latest
full update. On first attach it can send topology followed by a full frame. On
resume with `last_seen_seq`, retained updates replay in order; if the requested
sequence is older than the ring, the client receives a resync/full reset.

The Mux Client uses the same Pane View materialization path for the single local
pane that future remote clients use.

## Image cache events

The protocol reserves out-of-band image cache events:

- `ImagePut` for small payloads.
- `ImagePutStart`, `ImagePutChunk`, and `ImagePutComplete` for large payloads.
- `ImageEvict` for server-side payload deletion.

Image payloads carry width, height, byte length, format, digest, and bytes.
Render frames carry placement references. Renderer LRU eviction is local; it
does not mean the server evicted the image. If a later frame references a
locally missing image, the client sends `ImageCacheMiss` and skips that
placement until the server re-sends the payload.

The renderer exposes explicit image-cache event application. The current
in-process compatibility path can still visit snapshot image payloads while VT
publication is being moved to ordered image events.

## Transport roadmap

The build order is intentionally incremental:

1. Single binary with `MuxClient` over `LocalDomain`, plus `ProtocolDomain` over
   `InProcessTransport` for transport conformance tests.
2. Two local processes over Unix domain sockets.
3. SSH-tunneled TCP or UDS, plus TLS where needed.
4. QUIC only when connection migration, stream multiplexing, or latency
   constraints justify the extra moving parts.

The same conformance suite should run against every transport. Phase 1 stops at
schema, codec, local Domain, protocol client adapter, and simulated replay.

## Flow control

Initial limits are constants in `seance-protocol`:

- `MAX_DECODED_MESSAGE_BYTES`
- `MAX_PTY_INPUT_BYTES`
- `MAX_PENDING_INPUT_BYTES_PER_CLIENT`
- `MAX_IMAGE_CHUNK_BYTES`
- `MAX_PENDING_OUTBOUND_BYTES_PER_CLIENT`
- `MAX_RETAINED_PANE_UPDATES`

Frame updates may be coalesced to the latest pane state, but losing a required
partial base forces a full frame. Topology, pane exit, protocol errors, and
lifecycle messages are never dropped. Resize commands coalesce to the latest
size. Slow clients resync to a full frame when possible; otherwise they detach
with a classified error.

## Persistence boundaries

Persistence is split by lifetime and threat model, not by implementation
convenience:

- Server in-memory ephemeral: live PTYs, VT state, dirty tracking.
- Server append-only on disk: optional scrollback, with explicit privacy policy.
- Server transactional on disk: session/topology metadata for browser-style cold
  restart, not live process recovery.
- Client static config: declarative TOML/KDL-style settings.
- Client dynamic preferences: per-user/per-machine state such as geometry and
  MRU lists.
- Client cache: recomputable data under the cache directory.

Server persists shared truth that survives clients. Client persists
per-user/per-machine state. Nothing persists what can be recomputed. Scrollback
persistence must account for secrets; private-pane and encryption-at-rest
policies are protocol-adjacent but not Phase 1 implementation work.

## Error taxonomy

Protocol errors are structured as
`ProtocolErrorPayload { kind, message, request_id, pane }`. Kinds include
version mismatch, unsupported capability, unknown message, bad route, stale
pane, need-full, frame too large, image too large, protocol corruption, pane
exit, transport EOF, clean detach, and server pane error.

## WezTerm ptymux lessons

The schema follows WezTerm ptymux in using an explicit ordered PDU stream,
request serials, server pushes, up-front compatibility negotiation, mapped
remote identities, attach via topology/current state, and heavy coalescing.
Séance intentionally uses min/max protocol versions instead of exact codec
equality and keeps VT Core as the only owner of live terminal state.
