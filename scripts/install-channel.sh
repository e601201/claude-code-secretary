#!/usr/bin/env bash
# secretary-channel をビルドし、Claude Code のユーザースコープ MCP サーバー "secretary" として登録する(Phase 10)。
# 何度実行しても同じ結果になる。外すときは: claude mcp remove -s user secretary
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
echo "building secretary-channel (release)…"
cargo build --release -p secretary-channel
BIN="$ROOT/target/release/secretary-channel"
if claude mcp get secretary >/dev/null 2>&1; then
  claude mcp remove -s user secretary >/dev/null 2>&1 || true
fi
claude mcp add -s user secretary -- "$BIN"
echo
echo "registered: secretary -> $BIN"
echo "channel として使うには、セッションをこう起動してください:"
echo "  claude --dangerously-load-development-channels server:secretary"
echo "(研究プレビュー中は自作 channel に --channels が使えないため。起動時の確認で「I am using this for local development」を選びます)"
