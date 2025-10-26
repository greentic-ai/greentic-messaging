.PHONY: all build check fmt lint test run-runner
all: build
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
	docker compose -f docker/docker-compose.yml up -d
stack-down:
	docker compose -f docker/docker-compose.yml down -v
nats-ui:
	open http://localhost:8222 || xdg-open http://localhost:8222 || true
run-nats-demo:
	NATS_URL=$${NATS_URL:-nats://127.0.0.1:4222} TENANT=$${TENANT:-acme} cargo run -p nats-demo
run-mock-telegram:
	RUST_LOG=info cargo run -p mock-telegram
run-mock-slack:
	RUST_LOG=info cargo run -p mock-slack
tunnel-telegram:
	bash tools/tunnel.sh 9081
tunnel-slack:
	bash tools/tunnel.sh 9082
run-ingress-telegram:
	RUST_LOG=info TENANT=$${TENANT:-acme} NATS_URL=$${NATS_URL:-nats://127.0.0.1:4222} \
	TELEGRAM_SECRET_TOKEN=$${TELEGRAM_SECRET_TOKEN:-dev} cargo run -p gsm-ingress-telegram
run-ingress-webchat:
	RUST_LOG=info NATS_URL=$${NATS_URL:-nats://127.0.0.1:4222} cargo run -p gsm-ingress-webchat
run-egress-webchat:
	RUST_LOG=info TENANT=$${TENANT:-acme} PLATFORM=$${PLATFORM:-webchat} NATS_URL=$${NATS_URL:-nats://127.0.0.1:4222} \
	cargo run -p gsm-egress-webchat
run-egress-teams:
	RUST_LOG=info TENANT=$${TENANT:-acme} \
	MS_GRAPH_TENANT_ID=$${MS_GRAPH_TENANT_ID} MS_GRAPH_CLIENT_ID=$${MS_GRAPH_CLIENT_ID} \
	MS_GRAPH_CLIENT_SECRET=$${MS_GRAPH_CLIENT_SECRET} NATS_URL=$${NATS_URL:-nats://127.0.0.1:4222} \
	cargo run -p gsm-egress-teams
run-ingress-slack:
	RUST_LOG=info TENANT=$${TENANT:-acme} SLACK_SIGNING_SECRET=$${SLACK_SIGNING_SECRET} \
	NATS_URL=$${NATS_URL:-nats://127.0.0.1:4222} cargo run -p gsm-ingress-slack
run-ingress-whatsapp:
	RUST_LOG=info NATS_URL=$${NATS_URL:-nats://127.0.0.1:4222} cargo run -p gsm-ingress-whatsapp

run-egress-slack:
	RUST_LOG=info TENANT=$${TENANT:-acme} SLACK_BOT_TOKEN=$${SLACK_BOT_TOKEN} \
	NATS_URL=$${NATS_URL:-nats://127.0.0.1:4222} cargo run -p gsm-egress-slack
run-egress-telegram:
	RUST_LOG=info TENANT=$${TENANT:-acme} TELEGRAM_BOT_TOKEN=$${TELEGRAM_BOT_TOKEN} \
	NATS_URL=$${NATS_URL:-nats://127.0.0.1:4222} cargo run -p egress-telegram
run-egress-whatsapp:
	RUST_LOG=info TENANT=$${TENANT:-acme} WA_PHONE_ID=$${WA_PHONE_ID} WA_USER_TOKEN=$${WA_USER_TOKEN} \
	WA_TEMPLATE_NAME=$${WA_TEMPLATE_NAME:-weather_update} WA_TEMPLATE_LANG=$${WA_TEMPLATE_LANG:-en} \
	NATS_URL=$${NATS_URL:-nats://127.0.0.1:4222} cargo run -p gsm-egress-whatsapp
run-ingress-teams:
	RUST_LOG=info TENANT=$${TENANT:-acme} NATS_URL=$${NATS_URL:-nats://127.0.0.1:4222} \
	cargo run -p gsm-ingress-teams
run-subscriptions-teams:
	RUST_LOG=info TENANT=$${TENANT:-acme} \
	MS_GRAPH_TENANT_ID=$${MS_GRAPH_TENANT_ID} MS_GRAPH_CLIENT_ID=$${MS_GRAPH_CLIENT_ID} \
	MS_GRAPH_CLIENT_SECRET=$${MS_GRAPH_CLIENT_SECRET} TEAMS_WEBHOOK_URL=$${TEAMS_WEBHOOK_URL} \
	NATS_URL=$${NATS_URL:-nats://127.0.0.1:4222} cargo run -p gsm-subscriptions-teams
run-mock-weather-tool:
	RUST_LOG=info cargo run -p mock-weather-tool
