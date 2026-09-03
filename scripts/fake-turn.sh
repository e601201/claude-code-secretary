#!/bin/sh
# 偽の hook イベントを本体へ順に投げ、状態遷移を目で確かめる。
#   ./scripts/fake-turn.sh          正常に終わるターン
#   ./scripts/fake-turn.sh --fail   テスト失敗で終わるターン
# Discord 由来のタグを付けるので、既定の追跡方針(discord)でも表示対象になる。
set -eu

PORT="${SECRETARY_PORT:-47831}"
TOKEN_FILE="${SECRETARY_TOKEN_FILE:-$HOME/Library/Application Support/com.nagatadaichi.tauriapp/token}"
TOKEN=$(cat "$TOKEN_FILE" 2>/dev/null || true)
SID="fake-$(date +%s)"
CWD="$(pwd)"
MODE="${1:-ok}"

send() {
  printf '%s' "$1" | curl -s -m 2 -o /dev/null -w "%{http_code} " \
    -X POST "http://127.0.0.1:${PORT}/hook" \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer ${TOKEN}" \
    --data-binary @-
  echo "$2"
}

base() { printf '"session_id":"%s","cwd":"%s","hook_event_name":"%s"' "$SID" "$CWD" "$1"; }

send "{$(base SessionStart),\"source\":\"startup\"}" "SessionStart"
send "{$(base UserPromptSubmit),\"prompt_id\":\"p1\",\"prompt\":\"<channel source=\\\"plugin:discord:discord\\\" chat_id=\\\"1\\\" message_id=\\\"1\\\" user=\\\"you\\\" user_id=\\\"1\\\" ts=\\\"2026-09-03T00:00:00Z\\\">\\nテストを直して\\n</channel>\"}" "UserPromptSubmit (thinking)"
sleep 1.5
send "{$(base PreToolUse),\"tool_name\":\"Bash\",\"tool_use_id\":\"t1\",\"tool_input\":{\"command\":\"cat README.md\"}}" "PreToolUse Bash cat (thinking)"
sleep 1
send "{$(base PostToolUse),\"tool_name\":\"Bash\",\"tool_use_id\":\"t1\",\"tool_input\":{\"command\":\"cat README.md\"},\"tool_response\":{\"stdout\":\"...\"}}" "PostToolUse"
send "{$(base PreToolUse),\"tool_name\":\"Edit\",\"tool_use_id\":\"t2\",\"tool_input\":{\"file_path\":\"src/main.ts\"}}" "PreToolUse Edit (working)"
sleep 2
send "{$(base PostToolUse),\"tool_name\":\"Edit\",\"tool_use_id\":\"t2\",\"tool_input\":{\"file_path\":\"src/main.ts\"}}" "PostToolUse"
send "{$(base PreToolUse),\"tool_name\":\"Bash\",\"tool_use_id\":\"t3\",\"tool_input\":{\"command\":\"bun test\"}}" "PreToolUse Bash bun test (working)"
sleep 1
send "{$(base PermissionRequest),\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"bun test\"}}" "PermissionRequest (waiting)"
sleep 2.5
if [ "$MODE" = "--fail" ]; then
  send "{$(base PostToolUseFailure),\"tool_name\":\"Bash\",\"tool_use_id\":\"t3\",\"tool_input\":{\"command\":\"bun test\"},\"error\":\"Exit code 1\\n3 tests failed\",\"is_interrupt\":false}" "PostToolUseFailure (error)"
  sleep 2
  send "{$(base PreToolUse),\"tool_name\":\"mcp__plugin_discord_discord__reply\",\"tool_use_id\":\"t4\",\"tool_input\":{\"chat_id\":\"1\",\"text\":\"すみません、テストが3件失敗しています。\"}}" "PreToolUse reply (working)"
  sleep 1
  send "{$(base PostToolUse),\"tool_name\":\"mcp__plugin_discord_discord__reply\",\"tool_use_id\":\"t4\",\"tool_input\":{\"chat_id\":\"1\",\"text\":\"すみません、テストが3件失敗しています。\"}}" "PostToolUse"
  send "{$(base Stop),\"stop_hook_active\":false,\"last_assistant_message\":\"Discord へ報告しました。\"}" "Stop (idle: 失敗があったので success にしない)"
else
  send "{$(base PostToolUse),\"tool_name\":\"Bash\",\"tool_use_id\":\"t3\",\"tool_input\":{\"command\":\"bun test\"},\"tool_response\":{\"stdout\":\"42 passed\"}}" "PostToolUse (thinking)"
  send "{$(base PreToolUse),\"tool_name\":\"mcp__plugin_discord_discord__reply\",\"tool_use_id\":\"t4\",\"tool_input\":{\"chat_id\":\"1\",\"text\":\"直しました！テストは42件すべて通っています。\"}}" "PreToolUse reply (working)"
  sleep 1
  send "{$(base PostToolUse),\"tool_name\":\"mcp__plugin_discord_discord__reply\",\"tool_use_id\":\"t4\",\"tool_input\":{\"chat_id\":\"1\",\"text\":\"直しました！\"}}" "PostToolUse"
  send "{$(base Stop),\"stop_hook_active\":false,\"last_assistant_message\":\"Discord へ返信しました。\"}" "Stop (success → idle)"
fi
sleep 4
send "{$(base SessionEnd),\"reason\":\"other\"}" "SessionEnd"
