default: test

# Build & run
check:
    cargo check --workspace

build:
    cargo build --workspace --all-targets

run:
    tools/run.sh

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

# Vendored toolchain setup (re-run after cargo clean)
setup:
    tools/setup-ghostty-src.sh
    tools/setup-themes.sh
    tools/setup-sysroot.sh

# Run every CI gate locally
ci: fmt-check clippy test md-check
