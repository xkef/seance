default: test

test:
    cargo nextest run --workspace

test-render:
    cargo nextest run -p seance-render-test

snap-review:
    cargo insta review -p seance-render-test
