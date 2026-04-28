default: test

# Build & run
check:
    cargo check --workspace

build:
    cargo build --workspace --all-targets

run:
    tools/run.sh

make-app:
    tools/make-app.sh

# Clean & rebuild
clean:
    cargo clean

# Terminal-parented `cargo run`. May hit macOS focus quirks (see tools/run.sh).
clean-run:
    cargo clean
    cargo run

clean-run-app:
    cargo clean
    tools/run.sh

# Skips vendor setup vs. clean-run-app; assumes setup has already been staged.
clean-make-app-run:
    cargo clean
    tools/make-app.sh
    open target/Seance.app

# Format & lint
fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Tests
test:
    cargo nextest run --workspace

test-render:
    cargo nextest run -p seance-render-test

# Phase A snapshot harness
snap-review:
    cargo insta review -p seance-render-test

snap-bless:
    INSTA_UPDATE=always cargo nextest run -p seance-render-test

# Markdown (CI gates **/*.md against both)
md-check:
    npx --yes prettier --check "**/*.md"
    npx --yes markdownlint-cli2 "**/*.md"

md-fmt:
    npx --yes prettier --write "**/*.md"

# Vendored toolchain setup (re-run after cargo clean or libghostty-vt bumps)
setup-ghostty-src:
    tools/setup-ghostty-src.sh

setup-themes:
    tools/setup-themes.sh

setup-sysroot:
    tools/setup-sysroot.sh

setup: setup-ghostty-src setup-themes setup-sysroot

# Run every CI gate locally
ci: fmt-check clippy test md-check
