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

- `REPO_WORKER_TRANSPORT=nats|http` (default `nats`).
- `REPO_WORKER_NATS_SUBJECT` (default `workers.repo-assistant`).
- `REPO_WORKER_HTTP_URL` (HTTP mode only).
- `REPO_WORKER_ID` (default `greentic-repo-assistant`).
- `REPO_WORKER_RETRIES` (small local retry count, default `2`).

## Worker client abstraction

`WorkerClient` trait hides transport; implementations:

- `NatsWorkerClient` (default, request/reply over NATS).
- `HttpWorkerClient` (optional, POST/JSON callback).
- `InMemoryWorkerClient` (tests).

`forward_to_worker` builds a `WorkerRequest`, sends it via a `WorkerClient`, and maps the resulting `WorkerResponse.messages` into `OutboundEnvelope`s for the channel context.
