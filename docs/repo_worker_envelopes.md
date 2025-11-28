# Worker envelopes (greentic-messaging)

Messaging forwards channel requests to generic workers using the `greentic:worker@1.0.0` envelope. The shape is domain-agnostic and matches the WIT types carried in `greentic-types`.

## Envelope shape (WorkerRequest / WorkerResponse)

- `version`: semantic string, currently `1.0`.
- `tenant`: `TenantCtx`.
- `worker_id`: defaults to `greentic-repo-assistant`, configurable.
- `correlation_id`, `session_id`, `thread_id`: optional for tracing and threading; generated when missing.
- `payload_json`: opaque JSON payload as a string.
- `timestamp_utc`: RFC3339/ISO8601 UTC (with `Z`).
- `messages`: list of `{kind, payload_json}` in the response; messaging maps each item to channel payload(s).

## Routing config

- Global map (recommended): `WORKER_ROUTES=worker_id=nats:workers.repo-assistant,greentic-store-assistant=http:https://store/worker`
  - transport `nats` => subject; transport `http` => URL.
- Single route fallback: `REPO_WORKER_TRANSPORT=nats|http` (default `nats`), `REPO_WORKER_NATS_SUBJECT` (default `workers.repo-assistant`), `REPO_WORKER_HTTP_URL` (required when `REPO_WORKER_TRANSPORT=http`), `REPO_WORKER_ID` (default `greentic-repo-assistant`).
- `REPO_WORKER_RETRIES` (small local retry count, default `2`).

## Worker client abstraction

`WorkerClient` hides the transport; implementations:

- `NatsWorkerClient` (default, request/reply over NATS).
- `HttpWorkerClient` (optional, POST/JSON callback; required only when enabled in config).
- `InMemoryWorkerClient` (tests/mocks).

`forward_to_worker` builds a `WorkerRequest`, sends it via a `WorkerClient`, and maps the resulting `WorkerResponse.messages` into `OutboundEnvelope`s for the channel context. Channel correlation/session/thread IDs are preserved when present; missing correlation IDs are filled with a UUID.

## Examples (global routing map)

```env
# Repo assistant (default)
WORKER_ROUTES=greentic-repo-assistant=nats:workers.repo-assistant

# Store assistants (brand-specific)
WORKER_ROUTES=greentic-store-assistant=nats:workers.store-assistant,\
greentic-store-assistant-zain=nats:workers.store-assistant-zain,\
greentic-store-assistant-meeza=http:https://store-meeza.local/worker
```

Routes are currently global; TenantCtx travels in the envelope so per-tenant routing can be added later without changing the schema.
