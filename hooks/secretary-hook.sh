#!/bin/sh
# Claude Code hook → Secretary (Phase 0 スパイク)
# stdin に届く hook の JSON をローカルのログサーバーへそのまま転送する。
# Claude Code の動作を決して妨げないため、サーバー不在でも必ず 0 で終了する。
PORT="${SECRETARY_PORT:-47831}"
curl -s -m 1 -X POST "http://127.0.0.1:${PORT}/hook" \
  -H "Content-Type: application/json" \
  --data-binary @- >/dev/null 2>&1
exit 0
