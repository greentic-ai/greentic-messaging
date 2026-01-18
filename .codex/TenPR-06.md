# TenPR-06 — Delete legacy providers and registries

GOAL  
Leave only pack-based provider logic.

TASKS

1) Delete:
   - legacy provider traits
   - legacy provider registries
   - legacy provider implementations

2) Ensure all ingress/egress uses:
   - AdapterRegistry
   - ProviderExtensionsRegistry

3) Update tests:
   - Remove legacy provider tests
   - Add one pack-based provider smoke test

4) Remove legacy feature flag entirely if unused

ACCEPTANCE
- No legacy provider code remains
- Pack adapters are the only delivery mechanism
