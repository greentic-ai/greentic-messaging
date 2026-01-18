# Messaging pack validation

This document lists diagnostics emitted by the messaging pack validators in
`greentic-messaging-validate`.

## Provider declarations

- `MSG_NO_PROVIDER_DECL` (error): Messaging pack does not declare any providers.
- `MSG_PROVIDER_NO_OPS` (error): A provider declaration has an empty `ops` list.
- `MSG_PROVIDER_CONFIG_PATH_EMPTY` (error): A provider declaration has an empty config schema path.
- `MSG_PROVIDER_SCHEMA_EMPTY` (error): A provider declaration has an empty provider type.

## Setup flow contracts

- `MSG_SETUP_ENTRY_MISSING` (error): Setup flow is declared but no `setup` entrypoint exists.
- `MSG_SETUP_PUBLIC_URL_NOT_ASSERTED` (warn): Setup flow exists, but provider config schema does not
  indicate a public webhook URL field.

## Subscription contracts

- `MSG_SUBSCRIPTIONS_DECLARED_BUT_NO_FLOW` (warn): Subscriptions are declared but no subscriptions
  flow is present.

## Secret requirements

- `MSG_SECRETS_REQUIREMENTS_NOT_DISCOVERABLE` (warn): Provider operations are declared but secret
  requirements are not discoverable in the manifest.
