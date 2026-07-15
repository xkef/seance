# Threat model

séance is a terminal emulator: its core job is to execute untrusted byte
streams' _display instructions_ while never letting them become _code or
capability_. The attacker in every scenario below already controls what a
program inside the terminal prints — `cat`-ing a malicious file, an SSH session
to a hostile host, or a compromised build script are all equivalent.

## Assets

- The user's machine: séance runs unsandboxed with the user's privileges.
- The clipboard: both its contents (exfiltration) and its write path (priming a
  paste-to-shell attack).
- Other panes' contents once the native mux (M6) lands: one pane's program must
  not read another pane's screen.
- The user's trust in what the screen shows: escape sequences that misrender or
  hide content enable social-engineering attacks.

## Attack surfaces

### VT byte stream (`seance-vt`, libghostty-vt)

The primary surface. Every byte a child process writes reaches the parser.
Concerns: memory corruption in state handling, unbounded allocation (scrollback,
kitty-graphics cache — capped at 320 MB), and response sequences (DA, DSR, OSC
52 replies) that echo attacker-influenced data back onto the PTY as if typed.
The `vt_feed` fuzz target under `fuzz/` exercises this path end-to-end (parse →
snapshot → responses).

### Escape-sequence capabilities

Sequences that do more than draw: OSC 52 (clipboard read/write), kitty graphics
(decompression, large payloads), title/notification sequences, and future mux
control sequences. Each is a policy decision, not just a parsing problem —
clipboard reads are the canonical exfiltration channel.

### Mux protocol (`seance-protocol`, `seance-mux-client`, `seance-mux-server`)

Postcard-envelope deltas over a `Transport`. A malicious or compromised peer
must not be able to desynchronize client state into out-of-bounds access, claim
another pane's identity, or exhaust memory via crafted snapshots. Deserialized
lengths and indices are untrusted input.

### Rendering (`seance-render`)

Attacker-influenced data reaches cosmic-text/swash shaping and the wgpu
pipelines (glyph atlas, image renderer). Malformed fonts are out of scope (fonts
come from the OS), but image payloads from kitty graphics are not.

### Supply chain

Dependencies from crates.io, one git dependency (libghostty-rs, pinned by rev),
the vendored ghostty source (pinned commit, see `tools/setup-ghostty-src.sh`),
and GitHub Actions. Controls: `cargo deny` (advisories, licenses, sources),
Dependabot, CodeQL, OpenSSF Scorecard, SHA-pinned actions, and least-privilege
workflow tokens.

## Out of scope

- Attacks requiring an already-compromised user account or machine.
- Malicious local configuration (`config.toml` is trusted user input).
- Denial of service against the terminal by a program the user chose to run,
  where the damage is limited to the terminal session itself (e.g. printing
  garbage until killed).

## Reporting

See [SECURITY.md](../SECURITY.md) for private vulnerability reporting.
