#!/usr/bin/env bash
# Admit client ids to a running server's node allow-list (the enforcement
# that require_node_auth turns on). One POST /admin/allowlist per id,
# authenticated with the server's admin bearer token.
#
#   ADMIN_TOKEN=... deploy/allowlist.sh https://fl.example.org:8080 site-1 site-2 site-3
#
# The identity each client must present is, by default, the shared token
# node-auth-token (conflux-node's built-in default). Override it:
#   IDENTITY_TOKEN=<t>        the shared token this batch must present
#   IDENTITY_FINGERPRINT=<f>  an mTLS cert SHA-256 fingerprint instead
# (set at most one; fingerprint wins if both are set).
set -euo pipefail

: "${ADMIN_TOKEN:?set ADMIN_TOKEN (the server admin bearer token)}"
admin_url="${1:?usage: allowlist.sh <admin-url> <client-id>...}"
shift
[ "$#" -gt 0 ] || { echo "give at least one client id" >&2; exit 1; }

# The endpoint's identity object is an internally-tagged enum. Build the
# JSON with printf so there is no quote-escaping to get wrong.
if [ -n "${IDENTITY_FINGERPRINT:-}" ]; then
  identity=$(printf '{"kind":"cert_fingerprint","fingerprint":"%s"}' "$IDENTITY_FINGERPRINT")
else
  identity=$(printf '{"kind":"shared_token","token":"%s"}' "${IDENTITY_TOKEN:-node-auth-token}")
fi

for id in "$@"; do
  body=$(printf '{"client_id":"%s","identity":%s}' "$id" "$identity")
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$admin_url/admin/allowlist" \
    -H "Authorization: Bearer $ADMIN_TOKEN" \
    -H 'content-type: application/json' \
    -d "$body")
  echo "  $id -> HTTP $code"
done
