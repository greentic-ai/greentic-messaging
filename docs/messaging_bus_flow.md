# Messaging bus flow (ingress ↔ egress)

Messaging stays transport-neutral by publishing normalised envelopes onto an internal bus. Runner/adapters remain outside this repo.

```
[Channel/Webhook] -> Ingress Gateway -> Bus (ChannelMessage) -> Egress -> [Channel]
```

- **Ingress**: validates the inbound request, converts it to `ChannelMessage`, and publishes to `ingress_subject(env, tenant, team, platform)`.
- **Egress**: consumes `OutMessage` or outbound envelopes, resolves the adapter, and publishes channel-ready payloads to `egress_subject(tenant, platform)`.
- **BusClient**: shared trait with in-memory and NATS implementations; tests use the in-memory bus, binaries use NATS.
- **Subjects**: helpers in `messaging-bus` enforce the naming convention so strings aren’t scattered. Configure env/tenant/team/platform as usual; team is already sanitised at ingress.
- **Config knobs**: `MESSAGING_INGRESS_SUBJECT_PREFIX` and `MESSAGING_EGRESS_OUT_PREFIX` let you override the prefixes if your deployment needs different bus subjects.
- **Out of scope**: runner/adapters/secrets execution lives in other services; this repo only pushes/pulls envelopes.
