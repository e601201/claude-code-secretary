#!/usr/bin/env bash
# 秘書へ中継された権限要求に答える(Phase 10)。request_id はアプリのログ [channel] permission_request に出る。
#   ./scripts/permission.sh <request_id> allow|deny
set -euo pipefail
TOKEN_FILE="$HOME/Library/Application Support/com.nagatadaichi.tauriapp/token"
PORT="${SECRETARY_PORT:-47831}"
if [ $# -ne 2 ]; then echo "usage: $0 <request_id> allow|deny" >&2; exit 2; fi
case "$2" in allow) ALLOW=true;; deny) ALLOW=false;; *) echo "second arg must be allow or deny" >&2; exit 2;; esac
TOKEN="$(tr -d '\n' < "$TOKEN_FILE")"
CODE="$(curl -s -o /tmp/secretary-perm.out -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/permission" \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  --data-binary "{\"request_id\":\"$1\",\"allow\":$ALLOW}")"
case "$CODE" in
  204) echo "$2 sent for $1";;
  *) echo "failed ($CODE): $(cat /tmp/secretary-perm.out)" >&2; exit 1;;
esac
