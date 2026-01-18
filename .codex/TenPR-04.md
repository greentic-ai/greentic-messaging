# TenPR-04 — Make runner pack-driven and message-selected

GOAL  
Runner must select flow dynamically based on incoming message.
No env-selected flow. No hardcoded platform logic.

TASKS

1) Introduce FlowRegistry:
   - Loaded from the same `--packs-root` as adapters
   - Supports multiple packs
   - Discovers flows declared in pack metadata

2) Flow selection rules (minimal, deterministic):
   - Input: ChannelMessage
   - Match by:
     platform
     optional channel / route key
   - Fallback: pack default flow

3) Update gsm-runner:
   - Subscribe to canonical ingress subjects
   - Deserialize ChannelMessage
   - Resolve flow via FlowRegistry
   - Execute flow
   - Emit OutMessage

4) Remove:
   - FLOW env var
   - PLATFORM env var selection logic
   - Any “single flow” assumptions

ACCEPTANCE
- One runner instance can execute flows from multiple packs
- Flow is chosen per message
