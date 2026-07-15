# Security policy

## Reporting a vulnerability

Report vulnerabilities privately through GitHub's private vulnerability
reporting on this repository:
<https://github.com/xkef/seance/security/advisories/new>

Do not open a public issue for a security problem. Reports are acknowledged on a
best-effort basis; this is a single-maintainer project.

## Scope

This repository is a GPU-rendered terminal emulator. Reports of interest:

- Ways for untrusted terminal output (escape sequences, kitty graphics payloads,
  OSC strings) to corrupt memory, escape the VT layer, or execute code in
  `seance-vt`, `seance-render`, or `seance-input`.
- Weaknesses in the mux protocol (`seance-protocol`, `seance-mux-client`,
  `seance-mux-server`) that let a malicious peer desynchronize state or read
  another pane's contents.
- Workflow or token-permission weaknesses in `.github/workflows/`.

## Supported versions

Only the current state of `main` is supported. There are no releases or
backports.
