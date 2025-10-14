#!/usr/bin/env bash
set -euo pipefail

PORT="${1:-9081}"
if command -v cloudflared >/dev/null 2>&1; then
  echo "Starting Cloudflared tunnel for localhost:${PORT}"
  cloudflared tunnel --url "http://localhost:${PORT}"
else
  echo "cloudflared not found. Install from https://developers.cloudflare.com/cloudflare-one/connections/connect-apps/install-and-setup/installation"
  echo "Or use https://localtunnel.github.io/www/ with: npx localtunnel --port ${PORT}"
fi
