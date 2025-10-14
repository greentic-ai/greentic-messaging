default := ["build"]

all:
    just build

build:
    cargo build

check:
    cargo check --all-targets

fmt:
    cargo fmt --all

lint:
    cargo clippy --all-targets -- -D warnings || true

test:
    cargo test

run-runner:
    RUST_LOG=info cargo run -p gsm-runner
