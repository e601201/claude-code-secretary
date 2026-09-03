# 状態機械の仕様（Phase 1 成果物）

- 確定日: 2026-09-03
- 実装: `crates/secretary-core`（Tauri 非依存。`cargo test -p secretary-core` で本仕様の規則を検証する）
- 根拠: `docs/spike-results.md` の観測結果と Claude Code hooks の公式仕様

この文書と実装が食い違ったら、実装かこの文書のどちらかを直す。両方が真実であることを保つ。

---

## 1. 処理の流れ

```text
hook JSON ──▶ HookEnvelope ──▶ HookEvent ──▶ SessionState::apply ──▶ Tracker::snapshot ──▶ UI
              (受け皿)         (型付き)       (セッション単位)         (複数セッションを束ねる)
```

- `HookEnvelope` は hook の JSON をそのまま受ける。未知のフィールドは `extra` に保持し、欠けたフィールドは `None` にする。
- `HookEvent` は状態機械が扱う型付きイベント。購読していないイベントは `Other` になり、生存確認にだけ使う。
- 時刻は呼び出し側が `Instant` で渡す。実装は時計を持たない。

---

## 2. 状態

| 状態 | 種別 | 意味 | 優先度 |
|---|---|---|---|
| `idle` | 持続 | 何もしていない | 0 |
| `thinking` | 持続 | 応答を考えている。読み取り系・補助系ツールの実行中を含む | 2 |
| `working` | 持続 | 変更や実行を伴うツールの実行中 | 3 |
| `waiting` | 持続 | 権限要求などユーザーの応答待ち | 5 |
| `success` | 一時 | ターンが失敗なく終わった直後。既定 3 秒 | 1 |
| `error` | 一時 | ツール失敗またはターン失敗の直後。既定 5 秒 | 4 |

優先度は複数セッションを束ねて 1 体のキャラクターに表示するときに使う。

---

## 3. ツールの分類

`classify(tool_name, tool_input)` が 4 種類に分ける。迷ったら「作業」に倒す。

| 分類 | 対象 | 表示 |
|---|---|---|
| Reading | Read, Grep, Glob, LS, WebFetch, WebSearch, TodoRead, NotebookRead。Bash のうち読み取りコマンドだけのもの | thinking |
| Auxiliary | ToolSearch, ListMcpResourcesTool, ReadMcpResourceTool, TaskOutput, Monitor, ListAgents | thinking |
| Reply | `mcp__` で始まり `discord` を含み `__reply` で終わるツール。観測名は `mcp__plugin_discord_discord__reply` | working。`tool_input.text` を吹き出しに使う |
| Working | Edit, Write, NotebookEdit, Agent, その他すべて。上記以外の Bash | working |

### Bash の読み取り判定

Claude は Read ツールではなく Bash の `cat` で読むことがある（観測済み）ため、コマンド文字列を見る。

1. `2>&1`、`>/dev/null`、`2>/dev/null`、`&>/dev/null` を取り除く。残りに `>` があれば書き込みとみなし Working。
2. `&&`、`||`、`;`、`|`、改行で区切り、各区間の先頭語を見る。先頭の `VAR=value` は読み飛ばす。パス付き（`/bin/ls`）は末尾だけを見る。
3. すべての区間の先頭語が読み取りコマンド一覧（cat, ls, head, tail, grep, rg, find, pwd, echo, wc, jq, cd, date など）に含まれれば Reading。
4. `git` は副コマンドが status, log, diff, show, rev-parse, ls-files, blame, describe, shortlog, reflog, remote, branch, tag のときだけ Reading。`-d` `-D` `-m` `-M` があれば Working。
5. 空のコマンドは Working。

例: `ls README* 2>/dev/null; cat README.md | head -200` は Reading。`touch /tmp/x && ls -la /tmp/x` は Working。`echo hi > f` は Working。

---

## 4. セッションの状態遷移

各セッションは次を持つ。

| 項目 | 内容 |
|---|---|
| `in_turn` | プロンプト受付から Stop までの間 true |
| `in_flight` | 実行中ツールの集合。`tool_use_id` をキーにする |
| `waiting` | 許可待ちのツール名 |
| `transient` | 一時状態とその期限 |
| `turn_had_failure` | このターンでツール失敗があったか |
| `turn_has_reply` | このターンで Discord 返信を捕捉したか |
| `speech` | 吹き出し文言とその種別 |
| `task_summary` | 直近プロンプトの先頭行 |
| `discord_origin` | Discord 由来のプロンプトを一度でも受けたか |
| `last_event_at` | 失効判定に使う |

### 4.1 状態の導出（毎回この順で判定）

```text
1. transient があり期限内        → その状態（success / error）
2. waiting がある               → waiting
3. in_flight に Working か Reply → working
4. in_turn                       → thinking
5. それ以外                      → idle
```

### 4.2 イベントごとの更新

| イベント | 更新 |
|---|---|
| SessionStart | セッションを初期化する（cwd は引き継ぐ） |
| SessionEnd | `ended` にする。Tracker が削除する |
| UserPromptSubmit | `in_turn = true`。`in_flight`、`waiting`、`transient`、`speech`、失敗フラグをすべてクリア。プロンプトが `<channel source="plugin:discord:discord"` で始まれば `discord_origin = true`。タグを剥がした本文の先頭行を `task_summary` にする |
| PreToolUse | `in_turn = true`。`waiting` と `transient` をクリア。分類して `in_flight` に追加。Reply なら `tool_input.text` を `speech`（Reply）にし `turn_has_reply = true` |
| PostToolUse | `tool_use_id`（無ければツール名の後方一致）で `in_flight` から除去。`waiting` をクリア（権限要求の文言も消す） |
| PostToolUseFailure | 除去。`waiting` クリア。`turn_had_failure = true`。`transient = error`（5 秒）。`speech` を「{tool} が失敗しました: {error 先頭行}」（System）にする |
| PermissionRequest | `waiting = tool_name`。`speech` を「{tool} の実行許可を待っています」（Permission）にする |
| PermissionDenied | 除去。`waiting` クリア。error にはしない。auto mode でしか発火しない |
| Notification | `notification_type == "permission_prompt"` かつ `waiting` が空のときだけ `waiting` を設定する（直近の in_flight のツール名）。`idle_prompt` などは無視 |
| Stop | `in_turn = false`。`in_flight` と `waiting` をクリア。失敗が無ければ `transient = success`（3 秒）、あれば何もしない。`turn_has_reply` が false なら `last_assistant_message` を `speech`（Assistant）にする |
| StopFailure | Stop と同様にクリアし、`transient = error`。`speech` を「応答に失敗しました ({error})」（System）にする |
| Other | `last_event_at` の更新のみ |

### 4.3 規則の根拠

- **Stop で `in_flight` を必ず空にする。** 権限が拒否された呼び出しは PreToolUse だけ発火し PostToolUse が来ない（観測済み）。放置すると working のまま残る。
- **新しいイベントは一時状態を即座に上書きする。** error 表示中に PreToolUse が来れば直ちに working。UI 側で最低表示時間を設けたければ UI で行う。
- **拒否は error にしない。** 拒否は Claude の失敗ではない。
- **Discord 返信本文を `last_assistant_message` より優先する。** Discord 由来ターンの `last_assistant_message` は「Discord への返信を送信しました…」という作業報告で、ユーザーに見せた言葉ではない（観測済み）。
- **PermissionRequest には `tool_use_id` が無い。** ツール名で扱う。
- **許可待ちが解消したら権限要求の文言は消す。** 用済みの「実行許可を待っています」が次の発言まで残らないようにする（PreToolUse / PostToolUse / PostToolUseFailure / PermissionDenied / Stop で解除）。

---

## 5. 複数セッションの扱い（Tracker）

- セッションは `session_id` 単位で管理する。SessionStart を見ていないセッションからイベントが来たら、その場で作る（hook 登録前から動いていたセッション対策）。
- **cwd では区別しない。** 開発用セッションと Discord 用セッションが同じディレクトリで並走した（観測済み）。
- 表示対象は `FollowPolicy` で決める。

| 方針 | 対象 | 用途 |
|---|---|---|
| `Discord`（既定） | `discord_origin` が true のセッション | 本来の用途 |
| `All` | 全セッション | 開発時、fake event の確認 |
| `Session(id)` | 指定した 1 つ | トレイメニューからの手動固定 |

- `cwd_prefixes` が空でなければ、方針に加えて cwd の前方一致でさらに絞る。
- 表示するセッションは、対象の中で `(状態の優先度, 最終イベント時刻)` が最大のもの。
- **失効**: `stale_after`（既定 30 分）イベントの無いセッションは削除する。SessionEnd が届かないセッションがある（観測済み）ため。
- 対象が 1 つも無ければ idle のスナップショットを返す。

---

## 6. スナップショット

UI に渡す唯一の構造。`ts-rs` が `src/generated/SecretarySnapshot.ts` を生成する。

| フィールド | 内容 |
|---|---|
| `status` | 6 状態のいずれか |
| `message` / `message_kind` | 吹き出し文言と出所（reply / assistant / permission / system）。無ければ UI が状態テンプレートを使う |
| `current_tool` | 実行中ツールの短い説明。Bash は `Bash: <コマンド先頭行 60 文字>`、ファイル系は `Edit: <path>` |
| `task_summary` | 直近プロンプトの先頭行（120 文字まで） |
| `pending_permission` | 許可待ちのツール名 |
| `session_id` / `session_label` | 表示中セッション。ラベルは cwd の末尾ディレクトリ名 |
| `tracked_sessions` | 表示対象と認識しているセッション数 |

文言は文字数で切り、超えた分は「…」にする（バイト境界で切らない）。

---

## 7. 設定値

| 項目 | 既定 | 場所 |
|---|---|---|
| `success_hold` | 3 秒 | `HoldConfig` |
| `error_hold` | 5 秒 | `HoldConfig` |
| `max_message_chars` | 120 | `HoldConfig` |
| `stale_after` | 30 分 | `TrackerConfig` |
| `follow` | `Discord` | `TrackerConfig` |
| `cwd_prefixes` | 空 | `TrackerConfig` |

---

## 8. 既知の限界と今後

- 手動 Deny 時にどのイベントが届くかは未観測。現状は Stop での掃除に頼る。
- 拒否だけで終わったターンは、失敗イベントが無いので success と表示される。
- Bash の読み取り判定はヒューリスティック。`sed -n`（読み取り）は Working、`xargs rm`（書き込み）は先頭語 xargs が一覧に無いので Working。安全側に倒れているが、読み取りを working と誤ることはある。
- `thinking` を「reading」と分けたくなったら、`ToolClass::Reading` を状態にも昇格させれば足りる。
- 「長い作業中」の文言（Phase 9）は、working が一定時間続いたことを Tracker 側で検出して出す。
