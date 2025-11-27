## Messaging bus flow

- Ingress normalises channel payloads to `ChannelMessage` and publishes to the bus subject built via `ingress_subject[_with_prefix]`.
- Egress consumes outbound envelopes, resolves adapters, and publishes channel-ready payloads to `egress_subject[_with_prefix]`.
- Subject prefixes can be overridden via `MESSAGING_INGRESS_SUBJECT_PREFIX` and `MESSAGING_EGRESS_OUT_PREFIX`.
- The shared `gsm-bus` crate provides the `BusClient` trait plus in-memory and NATS implementations used by tests and binaries.
