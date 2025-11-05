# Telegram Provider Quickstart

This adapter delivers Adaptive Cards through the Telegram Bot API. Use the
instructions below to get the Telegram contract test green locally.

## Prerequisites

- A Telegram bot created through [BotFather](https://t.me/botfather) with its
  token handy.
- A chat to receive test messages (private chat with your bot, group, or
  channel).

## Configure secrets

1. Copy the bot token and export it:

   ```bash
   export TELEGRAM_BOT_TOKEN=123456:ABCDEF
   ```

2. Identify the chat:
   - If you already know the numeric chat ID, export `TELEGRAM_CHAT_ID`.
   - Otherwise export the handle (`@username` or invite link slug) and let the
     helper resolve the ID:

     ```bash
     export TELEGRAM_CHAT_HANDLE=@yourchat
     cargo run --manifest-path scripts/Cargo.toml --bin telegram_setup \
       --token "$TELEGRAM_BOT_TOKEN" --handle "$TELEGRAM_CHAT_HANDLE" \
       --output .env
     ```

   The script prints and optionally persists `TELEGRAM_CHAT_ID=<value>`.

Optional extras:

- `TELEGRAM_SECRET_TOKEN` allows you to exercise ingress webhook validation.

## Run the contract test

```bash
make stack-up
make conformance-telegram
make stack-down
```

The test posts the weather card to the chat, fetches the message history, and
asserts on the rendered text. Artifacts and payloads are written to
`tools/playwright/output/` and `target/e2e-artifacts/`.

## Troubleshooting

- If the suite logs “failed to resolve handle”, double-check that the bot has
  been added to the target chat and has permission to read history.
- Telegram’s API returns `403` when privacy mode blocks messages in groups; use a
  direct chat or disable privacy mode in BotFather.
