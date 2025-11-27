# Repo worker envelopes (greentic-messaging)

Messaging forwards channel requests to the repo assistant via a generic worker envelope. The envelope is transport-neutral and does not embed repo domain types.

## Envelope shape

- `version`: semantic string, currently `1.0`.
- `tenant`: `TenantCtx`.
- `worker_id`: defaults to `greentic-repo-assistant`, configurable.
- `correlation_id`, `session_id`, `thread_id`: optional for tracing and threading.
- `payload`: opaque JSON passed to the repo worker.
- `timestamp_utc`: RFC3339 string.

Responses:

- `messages`: list of `{kind, payload}`; messaging maps each item to channel payload(s).
- other fields mirror the request (`version`, `tenant`, `worker_id`, ids, `timestamp_utc`).

## Routing config

- Global map (recommended): `WORKER_ROUTES=worker_id=nats:workers.repo-assistant,greentic-store-assistant=http:https://store/worker`
  - transport `nats` => subject; transport `http` => URL.
- Single route fallback: `REPO_WORKER_TRANSPORT=nats|http` (default `nats`), `REPO_WORKER_NATS_SUBJECT`, `REPO_WORKER_HTTP_URL`, `REPO_WORKER_ID`.
- `REPO_WORKER_RETRIES` (small local retry count, default `2`).

## Worker client abstraction

`WorkerClient` trait hides transport; implementations:

- `NatsWorkerClient` (default, request/reply over NATS).
- `HttpWorkerClient` (optional, POST/JSON callback).
- `InMemoryWorkerClient` (tests).

`forward_to_worker` builds a `WorkerRequest`, sends it via a `WorkerClient`, and maps the resulting `WorkerResponse.messages` into `OutboundEnvelope`s for the channel context.

## Examples (global routing map)

```env
# Repo assistant (default)
WORKER_ROUTES=greentic-repo-assistant=nats:workers.repo-assistant

# Store assistants (brand-specific)
WORKER_ROUTES=greentic-store-assistant=nats:workers.store-assistant,\
greentic-store-assistant-zain=nats:workers.store-assistant-zain,\
greentic-store-assistant-meeza=http:https://store-meeza.local/worker
```

All routes are global for now; TenantCtx travels in the envelope so future per-tenant routing can be layered in without changing the schema.
