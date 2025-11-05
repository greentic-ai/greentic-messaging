# WebChat Provider Quickstart

The WebChat provider is a first-party channel that speaks the same contract as
the external platforms but runs entirely locally. Use it to verify end-to-end
flows without external credentials.

## Prerequisites

- Docker (for NATS via `make stack-up`).
- A browser to open the hosted widget.

## Run the local stack

```bash
make stack-up
make run-ingress-webchat
make run-egress-webchat
FLOW=examples/flows/weather_telegram.yaml PLATFORM=webchat make run-runner
```

The runner publishes the sample weather flow, which the ingress serves through
`http://localhost:8080/widget.js`. Open that URL from any page and interact with
the widget (`data-tenant="acme"` by default).

## Observing messages

- Ingress logs show inbound messages and normalised events.
- Egress logs display the translated Adaptive Card payload.
- No Playwright artifacts are generated for WebChat; use your browser’s devtools
  for screenshots or leverage the tooling in `tools/renderers`.

## Cleanup

Stop the processes (Ctrl+C) and run `make stack-down` when finished.
