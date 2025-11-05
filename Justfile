default := ["build"]
stack_file := "docker/stack.yml"
e2e_args := "--features e2e -- --ignored --nocapture"

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
stack-up:
    STACK_FILE=${STACK_FILE:-{{stack_file}}}; \
    if [ ! -f "$${STACK_FILE}" ]; then \
      STACK_FILE=docker/docker-compose.yml; \
    fi; \
    docker compose -f "$${STACK_FILE}" up -d
stack-down:
    STACK_FILE=${STACK_FILE:-{{stack_file}}}; \
    if [ ! -f "$${STACK_FILE}" ]; then \
      STACK_FILE=docker/docker-compose.yml; \
    fi; \
    docker compose -f "$${STACK_FILE}" down -v
conformance:
    just conformance-slack
    just conformance-telegram
    just conformance-webex
    just conformance-whatsapp
    just conformance-teams
    echo "Conformance suite complete"
conformance-slack:
    cargo test -p gsm-egress-slack {{e2e_args}}
conformance-telegram:
    cargo test -p egress-telegram {{e2e_args}}
conformance-webex:
    cargo test -p gsm-egress-webex {{e2e_args}}
conformance-whatsapp:
    cargo test -p gsm-egress-whatsapp {{e2e_args}}
conformance-teams:
    cargo test -p gsm-egress-teams {{e2e_args}}
