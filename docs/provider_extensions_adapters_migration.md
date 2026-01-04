# Provider extensions → messaging adapters (migration note)

- Adapter discovery now prefers provider extensions embedded in pack manifests (`greentic.provider-extension.v1`).
- Packs that declare providers via the extension will surface adapters automatically in CLI/Gateway/Egress. Adapter names use `provider_type`; component id comes from `runtime.component_ref`; kind defaults to ingress-egress.
- Legacy `messaging.adapters` remains supported for compatibility, but triggers a warning. If both extension and legacy entries exist, the extension wins.
- Packs with neither provider extension nor `messaging.adapters` will not register adapters; the CLI prints a hint to include one of them.
- To make extension-only packs usable, ensure the manifest includes the provider extension inline payload (or a resolvable reference) listing your providers.
