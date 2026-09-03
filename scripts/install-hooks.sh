#!/bin/sh
# Phase 0 スパイク用: hook スクリプトを ~/.claude/hooks/ に配置し、
# ~/.claude/settings.json に Secretary 用の hook を登録する。何度実行しても同じ結果になる。
set -eu

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
HOOK_SRC="$REPO_DIR/hooks/secretary-hook.sh"
HOOK_DIR="$HOME/.claude/hooks"
HOOK_DST="$HOOK_DIR/secretary-hook.sh"
SETTINGS="$HOME/.claude/settings.json"

mkdir -p "$HOOK_DIR"
cp "$HOOK_SRC" "$HOOK_DST"
chmod +x "$HOOK_DST"
echo "installed: $HOOK_DST"

if [ -f "$SETTINGS" ]; then
  BACKUP="$SETTINGS.bak.$(date +%Y%m%d-%H%M%S)"
  cp "$SETTINGS" "$BACKUP"
  echo "backup:    $BACKUP"
fi

SETTINGS="$SETTINGS" HOOK_DST="$HOOK_DST" python3 - <<'PY'
import json, os

settings_path = os.environ["SETTINGS"]
hook_cmd = f"sh '{os.environ['HOOK_DST']}'"
events = [
    "SessionStart", "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse", "PostToolUse", "PostToolUseFailure",
    "PermissionRequest", "PermissionDenied", "Notification",
    "Stop", "StopFailure",
]

data = {}
if os.path.exists(settings_path):
    with open(settings_path, encoding="utf-8") as f:
        content = f.read().strip()
        data = json.loads(content) if content else {}

hooks = data.setdefault("hooks", {})
added, skipped = [], []
for event in events:
    groups = hooks.setdefault(event, [])
    already = any(
        "secretary-hook.sh" in h.get("command", "")
        for g in groups for h in g.get("hooks", [])
    )
    if already:
        skipped.append(event)
        continue
    groups.append({"hooks": [{"type": "command", "command": hook_cmd, "timeout": 3}]})
    added.append(event)

with open(settings_path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2, ensure_ascii=False)
    f.write("\n")

print("added:    ", ", ".join(added) or "(none)")
print("skipped:  ", ", ".join(skipped) or "(none)")
PY
echo "settings:  $SETTINGS"
