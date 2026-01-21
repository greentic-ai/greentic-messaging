# TenPR-03 — Enforce canonical NATS subject scheme

GOAL  
Ensure the entire system uses ONLY:
  greentic.messaging.ingress.*
  greentic.messaging.egress.*

TASKS

1) Remove usage of:
   - greentic.msg.in.*
   - greentic.msg.out.*
   - any legacy subject helpers

2) Centralize subject construction:
   - One module in libs (e.g. `messaging_subjects`)
   - Subjects derived from:
     env / tenant / team / platform
   - User stays in message payload, not subject

3) Update:
   - gsm-gateway publisher
   - gsm-runner subscribers
   - gsm-egress consumers

4) Add tests:
   - Subject construction is deterministic
   - No legacy subjects appear in logs/tests

ACCEPTANCE
- Ripgrep for `greentic.msg.` returns nothing (outside legacy feature)
