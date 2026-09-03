#!/bin/sh
# Claude Code hook → デスクトップ秘書
# stdin に届く hook の JSON を、アプリ内の HTTP サーバーへそのまま転送する。
# Claude Code の動作を決して妨げないため、サーバー不在でも必ず 0 で終了する。
PORT="${SECRETARY_PORT:-47831}"
TOKEN_FILE="${SECRETARY_TOKEN_FILE:-$HOME/Library/Application Support/com.nagatadaichi.tauriapp/token}"
TOKEN=$(cat "$TOKEN_FILE" 2>/dev/null || true)
curl -s -m 1 -X POST "http://127.0.0.1:${PORT}/hook" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  --data-binary @- >/dev/null 2>&1
exit 0
