# Phase 0 スパイク結果: Claude Code hook イベントの観測

- 開始日: 2026-09-03
- 目的: 6状態（idle / thinking / working / waiting / success / error）を導出できるシグナルが hooks から取れることを確かめる。

## 実行手順

1. ログサーバーを起動する（リポジトリのルートで）

   ```sh
   cargo run -p hook-logger
   ```

   `logs/hooks.jsonl` に全イベントが追記され、標準出力に1行要約が流れる。

2. hook を登録する（初回のみ。何度実行しても安全）

   ```sh
   ./scripts/install-hooks.sh
   ```

   `~/.claude/settings.json` に11イベント分の hook が追加され、実行前の設定は `settings.json.bak.<日時>` に退避される。2026-09-03 に実行済み。

3. 新しい Claude Code セッションを起動して操作する。Discord 経由の観測は次のコマンドで起動したセッションに DM を送る。

   ```sh
   cd ~/workspace/life
   claude --channels plugin:discord@claude-plugins-official
   ```

4. 観測後、`logs/hooks.jsonl` を見ながら下の確認項目を埋める。

   ```sh
   # イベント種別ごとの件数
   jq -r '.hook.hook_event_name' logs/hooks.jsonl | sort | uniq -c
   # Discord 由来のプロンプトだけ
   jq -c 'select(.hook.hook_event_name=="UserPromptSubmit" and (.hook.prompt|test("<channel")))' logs/hooks.jsonl
   ```

## 公式ドキュメントで確定した事項（実機確認不要）

| 事項 | 内容 |
|---|---|
| 存在するイベント | SessionStart, SessionEnd, UserPromptSubmit, PreToolUse, PermissionRequest, PostToolUse, PostToolUseFailure, Notification, Stop, StopFailure, SubagentStart/Stop, PreCompact ほか |
| 共通フィールド | `session_id`, `transcript_path`, `cwd`, `permission_mode`, `hook_event_name` |
| 並列ツールの対応づけ | PreToolUse / PostToolUse / PostToolUseFailure に `tool_use_id` が入る。PermissionRequest には入らない |
| ターン終了時の発言 | Stop に `last_assistant_message`（Claude の最終応答文）が入る。吹き出しの文言源として使える |
| ターン失敗 | StopFailure に `error`（rate_limit, overloaded など）が入る。Stop 自体には成否情報がない |
| ツール失敗 | PostToolUseFailure に `error`, `is_interrupt`, `duration_ms` が入る |
| 通知種別 | Notification に `notification_type`（例: `permission_prompt`）と `message` が入る |
| hook の型 | `command` のほかに `http`（JSON をそのまま POST）がある。HTTP 型は接続失敗が非ブロッキングエラー扱い |
| 非同期実行 | command 型は `async: true` で Claude をブロックしない。ただし順序保証が弱くなる |

## 確認項目（実機）

結果欄に「済 / 未 / 不可」と要点を書く。

| # | 確認項目 | 結果 | メモ |
|---|---|---|---|
| 1 | ターミナルから起動したセッションで SessionStart / UserPromptSubmit / PreToolUse / PostToolUse / Stop / SessionEnd が届く | 済 | `claude -p` の headless セッション3回で全6種を観測。ターン内のイベントには `prompt_id` も付く |
| 2 | Discord 由来のターンでも同じイベントが届く | 済 | 2026-09-03 11:10 に3ターン観測。UserPromptSubmit → PreToolUse / PostToolUse → Stop が欠けなく届いた |
| 3 | Discord 由来の UserPromptSubmit の `prompt` に `<channel source="discord"` が含まれる | 済（表記が異なる） | 実際は `<channel source="plugin:discord:discord" chat_id=… message_id=… user=… user_id=… ts=…>` で始まり、本文を挟んで `</channel>` で終わる |
| 4 | Discord へ返信するとき PreToolUse が発火する。`tool_name` の表記と `tool_input` の本文フィールド名 | 済 | `tool_name` は `mcp__plugin_discord_discord__reply`。`tool_input` は `chat_id` と `text`。PostToolUse の `tool_response` は `sent (id: …)` |
| 5 | PermissionRequest が発火する。Discord 側で Allow した後に PostToolUse が続く | 不可（この環境） | ユーザー設定が `permissions.defaultMode: "auto"` のため `touch /tmp/…` は自動承認され、Allow / Deny ボタン自体が出なかった（`permission_mode: "auto"` を確認）。再観測は `claude --permission-mode default --channels …` で行う。headless の自動拒否では PreToolUse だけが残る |
| 6 | Notification の `notification_type` にどんな値が来るか（permission_prompt, idle など） | 一部 | `idle_prompt`（message: "Claude is waiting for your input"）がターン終了の約60秒後に届いた。`permission_prompt` は auto mode のため未観測 |
| 7 | PostToolUseFailure が発火する条件（Bash の非0終了、ツールエラー、拒否） | 一部 | 権限拒否では発火しない（確認済み）。非0終了は未観測 |
| 8 | 並列ツール呼び出しで Pre と Post の `tool_use_id` が対応づく | 一部 | 単一呼び出しでは同じ `tool_use_id` が Pre / Post に入ることを確認。並列時の順序は未観測 |
| 9 | 他のセッション（このリポジトリで動く Claude Code）からのイベントも届き、`cwd` で区別できる | 済（cwd は不十分） | 開発用セッションと Discord 用セッションが同じ `cwd`（tauri-app）で並走し、イベントが交互に届いた。`session_id` では区別できるが `cwd` では区別できない。追跡規則は `<channel source="plugin:discord:discord"` の有無を主にする |
| 10 | サーバー停止中でも Claude Code の動作と表示に影響が出ない | 済 | hook スクリプトは接続拒否時も終了コード0で即終了する（手動確認）。対話モードでの表示は要確認 |
| 11 | HTTP 型 hook にした場合、サーバー停止中に画面へエラー表示が出るか | 未 | 出なければ Phase 5 で HTTP 型を採用 |
| 12 | 同期 command 型 hook による体感遅延の有無 | 済 | Discord ターンで Bash の Pre→Post は 0.04〜2.0 秒、返信ツールは 1.7〜2.0 秒で、hook 起因の遅延は見えない。プロンプトから最初のツールまでの 3.6〜6.1 秒はモデルの思考時間 |

## 観測ログの抜粋（2026-09-03、headless セッション）

`transcript_path` は省略。3回の `claude -p` 実行で観測した各イベントの代表例。

### SessionStart

```json
{ "hook_event_name": "SessionStart", "session_id": "feb245f3-…", "cwd": "/Users/nagatadaichi/workspace/tauri-app", "source": "startup" }
```

### UserPromptSubmit

```json
{ "hook_event_name": "UserPromptSubmit", "session_id": "feb245f3-…", "cwd": "…/tauri-app", "permission_mode": "default",
  "prompt_id": "d8b56f46-…", "prompt": "Run this shell command and then reply with only its output: echo secretary-spike-ok" }
```

### PreToolUse

```json
{ "hook_event_name": "PreToolUse", "session_id": "feb245f3-…", "prompt_id": "d8b56f46-…",
  "tool_name": "Bash", "tool_use_id": "toolu_01BHdutBfh7DSViYkmesHGUZ",
  "tool_input": { "command": "echo secretary-spike-ok", "description": "Echo the specified text" } }
```

### PostToolUse

```json
{ "hook_event_name": "PostToolUse", "session_id": "feb245f3-…", "prompt_id": "d8b56f46-…",
  "tool_name": "Bash", "tool_use_id": "toolu_01BHdutBfh7DSViYkmesHGUZ", "duration_ms": 607,
  "tool_input": { "command": "echo secretary-spike-ok", "description": "Echo the specified text" },
  "tool_response": { "interrupted": false, "isImage": false, "noOutputExpected": false, "stderr": "", "stdout": "secretary-spike-ok" } }
```

### Stop

```json
{ "hook_event_name": "Stop", "session_id": "feb245f3-…", "prompt_id": "d8b56f46-…",
  "stop_hook_active": false, "last_assistant_message": "secretary-spike-ok",
  "background_tasks": [], "session_crons": [] }
```

### SessionEnd

```json
{ "hook_event_name": "SessionEnd", "session_id": "feb245f3-…", "prompt_id": "d8b56f46-…", "reason": "other" }
```

### 権限拒否時の並び（run 3）

```text
10:40:20.939 SessionStart      source=startup
10:40:23.139 UserPromptSubmit  prompt="Run this shell command: touch /tmp/secretary-spike-file. …"
10:40:25.637 PreToolUse        tool=Bash tool_use_id=toolu_018…
10:40:27.128 Stop              last_assistant_message="denied"
10:40:27.207 SessionEnd        reason=other
```

PreToolUse の後に PostToolUse も PostToolUseFailure も来ていない。実行中ツールの集合はこの経路で漏れるので、Stop で必ず空にする。PermissionDenied は auto mode の拒否専用で、この例のような非 auto mode の自動拒否や手動 Deny では発火しない。

## Discord 経由の観測（2026-09-03 11:08〜11:11）

セッション `d5d3fa32`、cwd は `~/workspace/tauri-app`（プロジェクトの settings.local.json で discord プラグインを有効化して起動）。3つの依頼を送った。

### UserPromptSubmit（Discord 由来）

```json
{ "hook_event_name": "UserPromptSubmit", "session_id": "d5d3fa32-…", "prompt_id": "…", "permission_mode": "auto",
  "prompt": "<channel source=\"plugin:discord:discord\" chat_id=\"1543515052726812672\" message_id=\"1544892229422948352\" user=\".daichi.n\" user_id=\"3354…\" ts=\"2026-09-03T02:10:10.204Z\">\nこのプロジェクトのREADMEを読んで、内容を3行で要約して\n</channel>" }
```

### 返信ツールの PreToolUse / PostToolUse

```json
{ "hook_event_name": "PreToolUse", "tool_name": "mcp__plugin_discord_discord__reply", "tool_use_id": "toolu_…",
  "tool_input": { "chat_id": "1543515052726812672", "text": "作りました" } }
```

```json
{ "hook_event_name": "PostToolUse", "tool_name": "mcp__plugin_discord_discord__reply", "tool_use_id": "toolu_…",
  "tool_response": [ { "type": "text", "text": "sent (id: 1544892283764215818)" } ] }
```

### 3ターンの流れ

```text
11:10:10 UserPromptSubmit  <channel …> READMEを読んで3行で要約して
11:10:14 PreToolUse        Bash  "ls README*; cat README.md | head -200"     ← Read ツールではなく Bash で読んだ
11:10:14 PostToolUse       Bash  (0.04s)
11:10:16 PreToolUse        ToolSearch  select:mcp__plugin_discord_discord__reply ← 遅延ロードの補助ツール
11:10:16 PostToolUse       ToolSearch
11:10:21 PreToolUse        mcp__plugin_discord_discord__reply  text="README の要約（3行）: …"
11:10:23 PostToolUse       mcp__plugin_discord_discord__reply  (1.9s)
11:10:27 Stop              last_assistant_message="Discord への返信を送信しました。\n\n**送った内容**\n…"

11:10:43 UserPromptSubmit  <channel …> /tmp/secretary-spike-allow.txt を作って
11:10:47 PreToolUse        Bash  "touch /tmp/secretary-spike-allow.txt && ls -la …"  ← 権限確認なしで実行（auto mode）
11:10:48 PostToolUse       Bash  (1.6s)
11:10:51 PreToolUse        mcp__plugin_discord_discord__reply  text="作りました"
11:10:52 PostToolUse       mcp__plugin_discord_discord__reply
11:10:54 Stop              last_assistant_message="`/tmp/secretary-spike-allow.txt` を空ファイルとして作成し、…"

11:11:17 UserPromptSubmit  <channel …> /tmp/secretary-spike-deny.txt を作って
11:11:22 PreToolUse        Bash  "touch /tmp/secretary-spike-deny.txt && …"          ← 同じく自動承認
11:11:24 PostToolUse       Bash
11:11:27 PreToolUse        mcp__plugin_discord_discord__reply  text="拒否されませんでした。…"
11:11:29 PostToolUse       mcp__plugin_discord_discord__reply
11:11:33 Stop
```

### この観測から分かった設計上の事実

- **hook の設定変更は稼働中のセッションにも即時反映される。** 登録直後から、登録作業をしていた開発用セッション自身のイベントも届き始めた。再起動は不要。
- **Discord 由来かどうかは `prompt` の先頭で判別できる。** `<channel source="plugin:discord:discord"` が目印。
- **Discord への返信本文は `mcp__plugin_discord_discord__reply` の `tool_input.text` で取れる。** 吹き出しの第一の文言源として使える。
- **Discord 由来ターンの `last_assistant_message` はユーザーに見せた言葉ではない。** 「Discord への返信を送信しました…」のような作業報告になる。吹き出しには返信ツールの本文を優先し、`last_assistant_message` は Discord を介さないターンの代替にする。
- **Claude は読み取りにも Bash を使う。** `cat README.md` のように Bash で読むため、ツール名だけで working / thinking を分けると読み取りが working と表示される。Bash はコマンドの先頭語（cat, ls, head, grep, find など）で読み取り系を判定する。
- **MCP ツールの初回使用前に `ToolSearch` が入る。** 補助ツールとして thinking 扱いにする。
- **開発用セッションと Discord 用セッションは同じ cwd で並走する。** `cwd` による追跡は使えず、`session_id` 単位の管理と Discord 由来判定が必須。
- **SessionEnd が届かないセッションがある。** 観測中の6セッションのうち3つに SessionEnd が無い。一定時間イベントの無いセッションは失効させる。
- **auto mode では権限確認がほとんど発生しない。** waiting 状態は稀になる。分類器が拒否したときは PermissionDenied が発火する（購読済み）。
- **Notification は無操作時にも届く。** ターン終了の約60秒後に `notification_type: "idle_prompt"` が来た。権限用の `permission_prompt` 以外の通知は状態に影響させない。
- **SessionEnd の `reason` に `prompt_input_exit` がある。** 対話セッションを Ctrl+C などで抜けたときの値。

## 6状態への対応表（確定版）

Phase 1 でこの表を `docs/plan.md` 5.2 節へ反映する。

Discord 経由の観測まで反映した版。waiting と error の行だけ、権限確認とコマンド失敗の再観測で裏付けが必要。

| 状態 | 導出に使うシグナル | 備考 |
|---|---|---|
| idle | SessionStart、Stop の一時状態が明けた後、SessionEnd、一定時間無音 | SessionEnd が来ないことがあるので無音タイムアウトも持つ |
| thinking | UserPromptSubmit、読み取り系ツール（Read / Grep / Glob / WebFetch / ToolSearch）の PreToolUse、Bash の読み取りコマンド（cat / ls / head / grep / find など）の PreToolUse、実行中集合が空になった PostToolUse | 観測済み |
| working | 変更・実行系ツール（Edit / Write / Agent / それ以外の Bash / `mcp__plugin_discord_discord__reply`）の PreToolUse | `tool_use_id` で集合管理。観測済み |
| waiting | PermissionRequest。Notification(`notification_type=permission_prompt`) を補助 | auto mode では稀。`--permission-mode default` で要再観測 |
| success | ターン内に失敗が無い Stop | 吹き出しは返信ツールの `text` を優先し、無ければ `last_assistant_message`。観測済み |
| error | PostToolUseFailure、StopFailure | 未観測。権限拒否は error にせず thinking へ戻す |

## 結論と次のアクション

### 2026-09-03 時点の結論

- **hooks はイベント源として成立する。** ターミナル起動の headless セッションと、Discord 経由の対話セッションの両方で、主要イベントが欠けなく届いた。
- **Discord 由来の判定、返信本文の取得、並列呼び出しの対応づけ、ターンの区切り**は、それぞれ `<channel source="plugin:discord:discord"`、`mcp__plugin_discord_discord__reply` の `tool_input.text`、`tool_use_id`、`prompt_id` で行える。
- **cwd による追跡は使えない。** 開発用セッションと Discord 用セッションが同じディレクトリで並走した。追跡は `session_id` 単位とし、Discord 由来判定を主規則にする。
- **auto mode のため waiting は稀。** 権限確認まわり（項目5, 6）とコマンド失敗（項目7）は未観測のまま Phase 1 へ進み、設計は公式ドキュメントの仕様に基づく。
- **hook スクリプトは Claude Code を妨げない。** サーバー不在でも即終了し、稼働中の観測でも遅延は見えなかった。

### 残りの観測（任意。Phase 1 と並行で可）

1. 権限確認の観測: `claude --permission-mode default --channels plugin:discord@claude-plugins-official` で起動し、Discord から「/tmp に空ファイルを作って」を送って Allow、もう一度送って Deny する。PermissionRequest、Notification、手動 Deny 時に届くイベントを記録する（項目5, 6）。
2. コマンド失敗の観測: Discord から「`bun run this-script-does-not-exist` を実行して結果を一言で教えて」を送る（項目7）。
3. HTTP 型 hook の観測: `~/.claude/settings.json` の PreToolUse だけを `{"type":"http","url":"http://127.0.0.1:47831/hook","timeout":2}` に差し替え、サーバーを止めた状態で1ターン操作し、画面にエラー表示が出るかを見る（項目11）。

### 次のアクション

Phase 1（状態と遷移の設計）へ進む。`docs/plan.md` の 5.2 節と 5.4 節は本記録の確定版に合わせて更新済み。
