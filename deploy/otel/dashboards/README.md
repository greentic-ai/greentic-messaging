# Grafana Dashboards & Trace Exemplars

This folder contains ready-to-import Grafana dashboards that surface the OTLP
metrics published by Greentic. Each dashboard assumes:

- `Prometheus` (or AMP, Thanos, etc.) stores the scraped metrics.
- Tempo (or any OTLP-compatible trace back-end) is available for trace drill-down.
- Exemplars are being recorded for the latency/counter series (the provided
  instrumentation already attaches them when `ENABLE_OTEL=true`).

## Importing Dashboards

1. In Grafana, add a **Prometheus** data source (UID recommended:
   `prometheus`) pointing at your metrics backend.
2. Add a **Tempo** data source (UID recommended: `tempo`) pointed at your trace
   backend.
3. Import each JSON dashboard (`Messages Golden Path`, `DLQ Heatmap`,
   `Rate Limit & Backpressure`, `Latency SLOs`) and, when prompted, map the
   data sources to the instances created in steps 1 and 2.

Each panel already has exemplar links configured:

- Hover the exemplar markers on a time-series visualization and use the
  **View Trace** link. The link opens Grafana Explore with a Tempo query that
  filters by the exemplar’s `trace_id`.
- Ensure Tempo is configured to query by the `trace_id` label (the default
  matcher is `{trace_id="<value>"}`).

If you customise data-source UIDs, update the dashboard templating variables
(`datasource_prometheus` / `datasource_tempo`) after import.

## Enabling Exemplars

To see exemplar markers, the metrics pipeline must export exemplars:

1. Set `ENABLE_OTEL=true` for the workload.
2. Point `OTEL_EXPORTER_OTLP_ENDPOINT` at the collector (see `deploy/otel/`).
3. Ensure the collector forwards metrics using OTLP/HTTP or gRPC with exemplars
   enabled. The supplied collector configs already do this for Prometheus
   Remote Write.
4. Prometheus must be configured to ingest exemplars (e.g. `--enable-feature=exemplar-storage`).

Once deployed, Grafana will show dotted marks over the latency/throughput
charts; click them to pivot from metrics to the originating trace for faster
incident response.
