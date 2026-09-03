#!/usr/bin/env bash
# 秘書からの指示として文字列を Claude Code のセッションへ送る(Phase 10)。
# 送り先はアプリが選ぶ(表示中のセッション、無ければ最新の接続)。
#   ./scripts/say.sh "テストを走らせて"
set -euo pipefail
TOKEN_FILE="$HOME/Library/Application Support/com.nagatadaichi.tauriapp/token"
PORT="${SECRETARY_PORT:-47831}"
if [ $# -lt 1 ]; then echo "usage: $0 <text>" >&2; exit 2; fi
if [ ! -f "$TOKEN_FILE" ]; then echo "token not found: $TOKEN_FILE (アプリを一度起動してください)" >&2; exit 1; fi
TOKEN="$(tr -d '\n' < "$TOKEN_FILE")"
BODY="$*"
CODE="$(curl -s -o /tmp/secretary-say.out -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/say" \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: text/plain; charset=utf-8' --data-binary "$BODY")"
case "$CODE" in
  200) echo "sent to $(cat /tmp/secretary-say.out)";;
  *) echo "failed ($CODE): $(cat /tmp/secretary-say.out)" >&2; exit 1;;
esac
