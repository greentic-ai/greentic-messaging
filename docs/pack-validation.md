# Messaging pack validation (provider pack contract)

This document is the single source of truth for how provider packs declare the
messaging validator.

## Validator world

`greentic:pack-validate/pack-validator@0.1.0`

## Distribution references

- OCI component: `ghcr.io/greentic-ai/validators/messaging:<version>`
- Pack bundle: `dist/validators-messaging.gtpack`

## Canonical extension declaration

Paste this into your provider pack manifest (pack.yaml/manifest.yaml):

```yaml
extensions:
  greentic.messaging.validators.v1:
    kind: greentic.messaging.validators.v1
    version: "1.0.0"
    inline:
      validators:
        - id: greentic.validators.messaging
          world: "greentic:pack-validate/pack-validator@0.1.0"
          component_ref: ghcr.io/greentic-ai/validators/messaging:__PACK_VERSION__
```

## Strict mode / pinning

For production, prefer digest pins once they are available:

`ghcr.io/greentic-ai/validators/messaging@sha256:<digest>`

This avoids surprises when a tag is moved or republished.
