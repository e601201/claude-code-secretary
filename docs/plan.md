# Claude Code デスクトップ秘書 計画書（改訂版）

- 改訂日: 2026-09-03
- 改訂理由: 初版のレビューを反映し、「Claude Codeからイベントを取り出す方法」「Rustの役割」「対象OS」「状態遷移の規則」を具体化した。

---

## 0. 改訂の要点

初版からの主な変更点は次のとおり。

- **イベント源をClaude Code hooksに確定**し、最初に半日の検証スパイク（Phase 0）を置く。
- **Secretary CoreをTauriのRust側に置く。** イベント受信、状態機械、セッション追跡、プロトコル型定義はRustが担い、TypeScriptは描画に専念する。
- **通信方式を確定。** Tauriアプリ（Rust）がlocalhostでHTTPサーバーを持ち、hookがPOSTする。WebSocketは常駐クライアントが必要になった時点で追加する。
- **対象OSはmacOS優先。** Windows対応は最終フェーズに置き、macOS固有の制約を設計に織り込む。
- **キャラクターは1枚絵と手続き的な動きでMVPを作る。** 多フレームのスプライトは後回しにする。
- **複数セッションと並列ツール呼び出しの扱いを定義**する。
- **「話しかける」機能はPhase 10として定義**し、プロトコルに逆方向メッセージを用意する（2026-09-03 完了）。
- **リポジトリは現在の単一Tauriアプリ構成を維持**する。モノレポ化はしない。

---

## 1. プロジェクト概要

### 1.1 コンセプト

Claude Codeを、デスクトップ上に常駐する「キャラクター秘書」として可視化する。

既存のDiscord経由のClaude Code操作は維持し、Claude Codeの状態や実行結果をキャラクターのアニメーション、吹き出し、ステータスUIとして表現する。

最終的な体験は次の一文に集約される。

> 実際にClaude Codeが仕事をしている間、デスクトップ上の秘書がそれを見守り、仕事が終わると報告してくれる。

### 1.2 前提となる既存環境

設計はこの前提に基づく。前提が変わった場合はこの節を更新する。

| 項目 | 内容 |
|---|---|
| Claude Codeの起動方法 | ターミナルで対話セッションを起動する。Discord連携時は `claude --channels plugin:discord@claude-plugins-official` で起動する |
| Discord連携 | 公式プラグイン `discord@claude-plugins-official` v0.0.4。Bun上で動くMCPサーバーで、Discordの受信メッセージを `<channel source="plugin:discord:discord" chat_id=… user=… ts=…>` で始まるターンとして稼働中のセッションへ流し込む。Claudeは `mcp__plugin_discord_discord__reply` ツール（引数 `chat_id`, `text`）で返信する（実機で確認） |
| 権限要求の扱い | プラグインは権限要求をDiscordへ転送し、Allow / Denyボタンで応答できる（`claude/channel/permission` capability） |
| プラグインのインストール範囲 | `~/workspace/life` と本リポジトリの両方でプロジェクトスコープ有効。Discord用セッションはどのディレクトリからでも起動されうるため、追跡は `cwd` に依存しない |
| 開発機 | macOS（Apple Silicon）。Rust stable、Tauri v2、Bun |
| 権限モード | ユーザー設定が `permissions.defaultMode: "auto"`。安全な操作は分類器が自動承認するため、権限確認（waiting）は稀にしか発生しない |
| 既存のhook | herdrがSessionStartにhookを登録済み。本プロジェクトのhookは別スクリプトとして追加し共存させる |

重要なのは、**Discord Botが別プロセスでClaude Codeを起動しているのではなく、Discordのメッセージが通常の対話セッションのターンとして処理される**点である。したがって、Claude Codeのhooksがそのままイベント源になる。2026-09-03のPhase 0で実機確認済み（`docs/spike-results.md`）。

### 1.3 基本構成

```text
   ┌──────────────┐
   │   Discord    │  ユーザーの指示
   └──────┬───────┘
          │
          ▼
   ┌──────────────────────────────┐
   │  Claude Code 対話セッション    │
   │   └ discord プラグイン (MCP)  │  受信メッセージを <channel> として注入
   └──────┬───────────────────────┘
          │ hooks (UserPromptSubmit / PreToolUse / PostToolUse /
          │        PermissionRequest / Stop ...)
          ▼
   ┌──────────────────────────────┐
   │  secretary-hook (小さなCLI)   │  stdinのJSONをそのままHTTP POST
   └──────┬───────────────────────┘
          │ HTTP POST 127.0.0.1:<port>/hook
          ▼
   ┌──────────────────────────────┐
   │  Tauri アプリ (Rust側)        │
   │   Secretary Core             │
   │    ├ HTTPサーバー             │
   │    ├ セッション追跡           │
   │    ├ 状態機械                 │
   │    └ メッセージ生成           │
   └──────┬───────────────────────┘
          │ Tauri event (state snapshot)
          ▼
   ┌──────────────────────────────┐
   │  Tauri webview (TypeScript)  │
   │   キャラクター / 吹き出し /   │
   │   ステータスパネル            │
   └──────────────────────────────┘
```

### 1.4 役割分担

| 要素 | 役割 |
|---|---|
| Discord | Claude Codeへの操作と入力。従来どおり |
| Claude Code | 実際の開発作業を行う頭脳 |
| hooks + secretary-hook | Claude Codeの出来事を外部へ通知する唯一の経路 |
| Secretary Core（Rust） | イベントを受け取り、状態と表示内容を決める |
| Tauri webview（TypeScript） | 決められた状態を描くだけの薄い層 |

---

## 2. 技術構成

| 部分 | 技術 |
|---|---|
| デスクトップアプリ | Tauri v2 |
| バックエンドロジック | Rust（Secretary Core、HTTPサーバー、状態機械） |
| フロントエンド | Vite + TypeScript。フレームワークなしで開始 |
| キャラクター描画 | DOMの `<img>` とCSS transform。必要になればCanvasへ移行 |
| 状態管理 | Rust側が唯一の真実。フロントはスナップショットを受け取って描画 |
| Rust ⇄ TS の型共有 | `ts-rs` でRustの型からTypeScript型を生成 |
| 通信 | hook → Tauri: localhost HTTP POST。Rust → webview: Tauri event |
| hookクライアント | Phase 0はシェル + curl。Phase 5以降はRust製の小さなCLI `secretary-hook` |
| パッケージマネージャー | Bun（Discordプラグインの要件とも一致） |
| キャラクター素材 | 1枚絵PNG。後にポーズ差分や連番PNGを追加 |

### 2.1 Rustが担う責務

Rustで開発するという目的に合わせ、ロジックは可能な限りRust側へ寄せる。

- hookイベントの受信と検証（HTTPサーバー、トークン検査）
- セッション単位の状態追跡と、表示対象セッションの選択
- 状態機械（イベント → 状態、タイムアウト、優先度）
- 吹き出しメッセージの生成（Claudeの発言のミラー、テンプレート辞書）
- 設定ファイルの読み書き、ウィンドウ位置の永続化
- プロトコルの型定義と、TypeScript型の生成
- 上記すべてのユニットテスト

### 2.2 Tauriを選ぶ理由

- 透明で装飾のないウィンドウを設定だけで作れる
- 常に前面表示、クリック透過、全ワークスペース表示などマスコット向けの機能がAPIとして揃っている
- バックエンドがRustなので、本プロジェクトの目的と一致する
- Web技術でUIを作れるため、キャラクター表現の試行錯誤が速い
- 将来Windowsへも同じコードベースで展開できる

---

## 3. 目指すUI

通常はキャラクターだけがデスクトップに常駐する。Claude Codeが動作すると、キャラクターが状態に応じて反応する。

```text
macOS Desktop
──────────────────────────────────────────────

                         ┌───────────────────┐
                         │ Claude Code       │
                         │ ● Working         │
                         │ bun test          │
                         │ 42 / 42 passed    │
                         └───────────────────┘

                              💬
                         「テストを回しています……」

                            🧍
                           /│\
                           / \

──────────────────────────────────────────────
```

普段はキャラクターだけを表示し、必要なときにステータスパネルや吹き出しを表示する。終了や設定はメニューバーのアイコンから行う。

---

## 4. Phase 0 ― イベント源の検証スパイク

**最優先。工数は半日から1日。** キャラクター制作やUI実装に投資する前に、Claude Codeから必要なシグナルが本当に取れることを確かめる。

### 4.1 やること

1. ログを書くだけの小さなHTTPサーバーを用意する。Rustで書けばそのままSecretary Coreの種になる。
2. hookスクリプトを追加し、受け取ったJSONをそのままPOSTする。
3. Discordから指示を出し、ターミナルから指示を出し、それぞれ何が届くかを記録する。

hookスクリプトの例。既存のherdr用スクリプトとは別ファイルにする。

```sh
#!/bin/sh
# ~/.claude/hooks/secretary-hook.sh
# stdinのhook JSONをそのままローカルサーバーへ転送する。失敗しても必ず0で終了する。
curl -s -m 1 -X POST "http://127.0.0.1:47831/hook" \
  -H "Content-Type: application/json" \
  --data-binary @- >/dev/null 2>&1
exit 0
```

`~/.claude/settings.json` への登録例。ユーザースコープに置き、どのセッションからも届くようにする。

```json
{
  "hooks": {
    "SessionStart":        [{ "matcher": "*", "hooks": [{ "type": "command", "command": "sh ~/.claude/hooks/secretary-hook.sh", "timeout": 2 }] }],
    "SessionEnd":          [{ "matcher": "*", "hooks": [{ "type": "command", "command": "sh ~/.claude/hooks/secretary-hook.sh", "timeout": 2 }] }],
    "UserPromptSubmit":    [{ "hooks": [{ "type": "command", "command": "sh ~/.claude/hooks/secretary-hook.sh", "timeout": 2 }] }],
    "PreToolUse":          [{ "matcher": "*", "hooks": [{ "type": "command", "command": "sh ~/.claude/hooks/secretary-hook.sh", "timeout": 2 }] }],
    "PostToolUse":         [{ "matcher": "*", "hooks": [{ "type": "command", "command": "sh ~/.claude/hooks/secretary-hook.sh", "timeout": 2 }] }],
    "PostToolUseFailure":  [{ "matcher": "*", "hooks": [{ "type": "command", "command": "sh ~/.claude/hooks/secretary-hook.sh", "timeout": 2 }] }],
    "PermissionRequest":   [{ "matcher": "*", "hooks": [{ "type": "command", "command": "sh ~/.claude/hooks/secretary-hook.sh", "timeout": 2 }] }],
    "Stop":                [{ "hooks": [{ "type": "command", "command": "sh ~/.claude/hooks/secretary-hook.sh", "timeout": 2 }] }],
    "StopFailure":         [{ "hooks": [{ "type": "command", "command": "sh ~/.claude/hooks/secretary-hook.sh", "timeout": 2 }] }]
  }
}
```

herdrのSessionStart hookが既に存在するので、追記の際は配列に要素を足す形で統合する。この登録は `scripts/install-hooks.sh` が行う（バックアップ付き、再実行しても安全）。設定変更は稼働中のセッションにも即時反映され、再起動は不要だった（観測済み）。

hookには `type: "http"` もあり、Claude Code自身がJSONをそのままPOSTできる。ただし接続失敗が非ブロッキングエラーとして扱われるため、サーバー不在時に画面へ通知が出ないことを確認してから採用を判断する（4.2の確認項目）。

### 4.2 確認項目

以下は公式ドキュメントで確認できなかった、または実機で確かめる必要がある事項。結果を `docs/spike-results.md` に残す。

- [x] Discord由来のターンでも `UserPromptSubmit`、`PreToolUse`、`PostToolUse`、`Stop` が発火する（実機で確認）
- [x] `UserPromptSubmit` の `prompt` は `<channel source="plugin:discord:discord" …>` で始まり、Discord由来と判別できる（実機で確認）
- [x] Discordへの返信時に `PreToolUse` が発火する。`tool_name` は `mcp__plugin_discord_discord__reply`、`tool_input.text` に返信本文が入る（実機で確認）
- [ ] `PermissionRequest` が発火するか。Discord側でAllowした後に `PostToolUse` が続くか（auto modeでは自動承認されて観測できなかった。`--permission-mode default` で再観測）
- [ ] `PostToolUseFailure` が発火する条件（コマンド失敗、ツールエラー、拒否）
- [x] 対応づけの識別子は `tool_use_id`。PreToolUse / PostToolUse / PostToolUseFailure に含まれ、PermissionRequestには含まれない（公式ドキュメントと実機で確認）。並列時の順序は未確認
- [x] `Stop` に成否情報は無い。代わりに `last_assistant_message`（Claudeの最終応答文）が入る（実機で確認）
- [x] 共通フィールドは `session_id`、`transcript_path`、`cwd`、`permission_mode`、`hook_event_name`。ターン内のイベントには `prompt_id` も付く（実機で確認）
- [x] ターミナル起動のセッションから SessionStart / UserPromptSubmit / PreToolUse / PostToolUse / Stop / SessionEnd が届いた。`cwd` と `session_id` で区別できる（実機で確認）
- [x] 権限が拒否されたツール呼び出しは PreToolUse のみ発火し、PostToolUse も PostToolUseFailure も来ない（headlessで確認）。対話モードでの PermissionRequest / PermissionDenied の発火は未確認
- [ ] `type: "http"` のhookでサーバー不在時に画面へエラー表示が出るか

進捗の詳細と観測ログは `docs/spike-results.md` に記録する。

### 4.3 完了条件

上の確認項目に答えが出ており、6状態それぞれを導出できるシグナルの組み合わせが決まっている。導出できない状態があれば、Phase 1で状態の定義を見直す。

---

## 5. Phase 1 ― 状態と遷移の設計

**2026-09-03 完了。** 確定した仕様は `docs/state-machine.md`、実装は `crates/secretary-core`（Tauri非依存、ユニットテストとPhase 0の実ログ再生テスト付き）。以下はその要約。

### 5.1 MVPの6状態

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum AssistantState {
    Idle,      // 何もしていない
    Thinking,  // 応答を考えている、または読み取り系ツールを実行中
    Working,   // 編集・コマンド実行など変更を伴う作業中
    Waiting,   // 権限要求などユーザーの応答待ち
    Success,   // ターンが正常に終わった直後（一時状態）
    Error,     // ツール失敗やターン失敗（一時状態）
}
```

| 状態 | 種別 | キャラクター |
|---|---|---|
| idle | 持続 | 待機 |
| thinking | 持続 | 考える |
| working | 持続 | 作業 |
| waiting | 持続 | 待機 + 吹き出し（idleの絵を流用） |
| success | 一時（既定3秒） | 喜ぶ |
| error | 一時（既定5秒） | 困る |

将来の拡張候補: `reading`（thinkingから分離）、`testing`（Bashのコマンド内容から判定）、`sleeping`（長時間idle）、`notification`。

### 5.2 hookイベントから状態への対応

Phase 0の結果で修正する前提の初期案。

| hookイベント | 条件 | 状態 | 補足 |
|---|---|---|---|
| SessionStart | | idle | セッションを登録 |
| UserPromptSubmit | | thinking | 直後の思考区間を表す。ターン開始として記録 |
| PreToolUse | Read / Grep / Glob / WebFetch / WebSearch / ToolSearch など読み取り・補助系 | thinking | 実行中集合に追加 |
| PreToolUse | Bash のうち先頭語が cat / ls / head / tail / grep / find / git status など読み取り系 | thinking | ClaudeはReadではなくBashで読むことがある（観測済み）。コマンド先頭語で判定 |
| PreToolUse | Edit / Write / NotebookEdit / Agent / 上記以外の Bash など変更・実行系 | working | 実行中集合に追加 |
| PreToolUse | `mcp__plugin_discord_discord__reply` | working | `tool_input.text` を吹き出しに表示（観測済み） |
| PostToolUse | | 実行中集合から除去。空になれば thinking | 並列実行を考慮 |
| PostToolUseFailure | | error（一時）→ 元の状態へ | ターン内失敗として記録 |
| PermissionRequest | | waiting | 同一セッションの次の PreToolUse / PostToolUse / PostToolUseFailure / PermissionDenied / Stop で解除 |
| Notification | `notification_type` が `permission_prompt` | waiting | PermissionRequestの補助。対話モードで要確認 |
| PermissionDenied | auto modeでの拒否のみ発火 | 実行中集合から除去。空なら thinking | 拒否は error にしない。手動Denyでは発火しないため Stop での掃除が必須 |
| Stop | ターン内に失敗なし | success（一時）→ idle | 実行中集合を空にする。`last_assistant_message` を吹き出しに使える |
| Stop | ターン内に失敗あり | idle | errorは既に表示済み。実行中集合を空にする |
| StopFailure | | error（一時）→ idle | |
| SessionEnd | | idle。セッションを削除 | |

「thinking」に直接対応するイベントは存在しない。プロンプト送信からツール呼び出しまでの区間、およびツール呼び出しの合間として導出する。

### 5.3 遷移の規則

- **一時状態はタイマーで戻る。** successとerrorは既定の秒数で、その時点で本来あるべき持続状態へ戻る。
- **新しいイベントは一時状態を即座に上書きする。** たとえばerror表示中にPreToolUseが来れば直ちにworkingになる。
- **waitingは解除イベントが来るまで続く。** 秘書は待っている間、吹き出しで何の許可を待っているかを示し続ける。
- **アニメーションの割り込み。** 一時状態のアニメーションは途中で打ち切って良い。持続状態のループアニメーションは、状態が変わった時点でクロスフェードする。

### 5.4 複数セッションの扱い

hooksは稼働中のすべてのClaude Codeセッションから届く。開発中はこのリポジトリのセッションからも届く。

- Secretary Coreは `session_id` ごとに状態を持つ。
- 表示対象は「追跡対象セッション」に限定する。主規則は「`UserPromptSubmit` の `prompt` が `<channel source="plugin:discord:discord"` で始まるプロンプトを受け取ったことのあるセッション」を自動追跡すること。Phase 0で判別できることを確認済み。
- `cwd` による追跡は主規則にしない。開発用セッションとDiscord用セッションが同じディレクトリで並走することが観測されたため。設定ファイルの `follow.cwd` は補助的な手動指定として残す。
- トレイメニューから追跡対象セッションを手動で固定できるようにする。
- SessionEndが届かないセッションがある（強制終了など。観測済み）。一定時間イベントの無いセッションは失効させ、追跡対象から外す。
- 追跡対象が複数ある場合の表示は優先度で決める。 `waiting > error > working > thinking > success > idle`。
- 表示対象外のセッションのイベントもログには残す。

### 5.5 並列ツール呼び出し

Claude Codeは複数のツールを同時に呼ぶことがある。単一フラグではなく、セッションごとに「実行中ツール」の集合を `tool_use_id` をキーに持ち、PreToolUseで追加、PostToolUse / PostToolUseFailure / PermissionDeniedで削除する。集合が空になった時点でthinkingへ戻す。権限が拒否された呼び出しはPostToolUseが来ないため、Stopで必ず集合を空にする。ターンの区切りには `prompt_id` を使う。

---

## 6. Phase 2 ― Tauriの最小アプリ

**2026-09-03 完了。** 実装は `src-tauri/src/lib.rs` と `src/`。実機で確認した完了条件: Dockに出ない、メニューバーに項目が出る、透明ウィンドウにキャラクターだけが見える、ドラッグで動く、クリック透過と常に前面の切り替えが効く、終了後の再起動で位置が復元される。

実装時に分かったこと:

- **位置は論理ピクセルで保存する。** 非表示のウィンドウは倍率が1.0と報告されるため、物理ピクセルで保存・復元するとRetinaで2倍の位置に飛ぶ。`tauri-plugin-window-state` も同じ理由でずれたので使わず、`window-state.json` に論理座標を自前で保存する。
- **トレイはアイコンを必ず明示する。** アイコン無しだとmacOSでは幅ゼロの項目になり見えない。デバッグビルドでは「秘書」の文字も出して見つけやすくしている。
- **初回起動はカーソルのあるディスプレイの右下に置く。** 複数ディスプレイ環境で、見ていない画面に出て気づけない事故を防ぐ。保存位置がどの画面にも無いときも同じ処理に落とす。
- **キャラクター画像は `public/character/base.png`。** Viteの静的配信の都合で `assets/` ではなく `public/` に置く。無ければ `placeholder.svg` の仮キャラクターが出る。
- **フロントエンドの診断は Rust の標準エラー出力へ流す。** `frontend_log` コマンドで画像の読み込み結果などを `tauri dev` のログに出せる。画面を直接見られない環境でも動作確認できる。

以下は着手前の計画。

Claude Codeとの接続はまだ行わない。目標は「デスクトップにキャラクターが表示され、ドラッグで動かせる」こと。

### 6.1 macOS向けのウィンドウ設定

macOSで透明ウィンドウを使うには、Tauriの `macos-private-api` 機能フラグが必須。これはApp Store配布を不可能にするが、本プロジェクトは個人利用なので許容する。

`src-tauri/Cargo.toml`:

```toml
[dependencies]
tauri = { version = "2", features = ["macos-private-api", "tray-icon"] }
```

`src-tauri/tauri.conf.json` の要点:

```json
{
  "app": {
    "macOSPrivateApi": true,
    "windows": [
      {
        "label": "character",
        "title": "Secretary",
        "width": 240,
        "height": 320,
        "transparent": true,
        "decorations": false,
        "shadow": false,
        "alwaysOnTop": true,
        "resizable": false,
        "visibleOnAllWorkspaces": true,
        "skipTaskbar": true
      }
    ]
  }
}
```

Rust側でDockアイコンを隠す。`skipTaskbar` はWindows / Linux専用なので、macOSではこちらが必要。

```rust
app.set_activation_policy(tauri::ActivationPolicy::Accessory);
```

### 6.2 capabilityの追加

Tauri v2ではウィンドウ操作に権限が必要。`src-tauri/capabilities/default.json` に少なくとも次を追加する。正確な権限名は実装時にTauriのリファレンスで確認する。

- ウィンドウのドラッグ開始（`data-tauri-drag-region` に必要）
- クリック透過の切り替え（`set_ignore_cursor_events`）
- 常に前面表示の切り替え
- ウィンドウ位置の取得と設定
- イベントの購読

### 6.3 実装項目

- 透明ウィンドウと常に前面表示
- Dockアイコンの非表示、メニューバーのトレイアイコン（終了だけのメニュー）
- キャラクターの仮表示（1枚絵）
- ドラッグ移動と、終了時のウィンドウ位置の保存と復元
- クリック透過のON / OFF（トレイメニューから切り替え）
- 全ワークスペースでの表示

### 6.4 完了条件

```text
アプリ起動
   ↓
Dockにアイコンが出ず、メニューバーにアイコンが出る
   ↓
透明なウィンドウにキャラクターだけが見える
   ↓
ドラッグで移動でき、再起動後も同じ位置に出る
   ↓
トレイメニューから終了できる
```

---

## 7. Phase 3 ― キャラクター表示と動き

**2026-09-03 完了。** 実装は `src/character/character.ts` と `src/styles.css`、Rust側は `src-tauri/src/bridge.rs`。Rust が `secretary://snapshot` イベントで `SecretarySnapshot` を流し、フロントは `data-state` 属性とバッジを切り替えるだけ。動きはすべてCSSアニメーション。状態ごとの差分画像は `public/character/<state>.png` があれば自動で使う。デバッグ用にトレイの「デバッグ」サブメニューと `SECRETARY_DEMO=1` の巡回デモを用意した。

### 7.1 方針

現在の素材は1枚絵。多フレームのスプライトを最初から作るのは工数が最大の非エンジニアリング作業になるため、MVPでは**1枚絵 + 手続き的な動き + 状態バッジ**で「生きている」感じを作る。

| 状態 | 動き | バッジ |
|---|---|---|
| idle | ゆっくりした上下の呼吸 | なし |
| thinking | 軽い左右の傾きと、頭上に「…」 | 💭 |
| working | 小刻みな上下動と、少し前傾 | ⚙ |
| waiting | idleの動きと、吹き出しの点滅 | 🔐 |
| success | 一度大きく跳ねる | ✨ |
| error | 横に小さく震えて、少し縮む | ⚠ |

実装はCSSアニメーションと `transform` で足りる。Canvasは使わない。

### 7.2 素材の拡張手順

1. 1枚絵で全状態を動かす（Phase 3）
2. 表情差分（喜び、困り）を2枚追加し、successとerrorで切り替える
3. 必要なら連番PNGでアニメーションを追加する。形式は「状態ごとのディレクトリに連番PNG」に統一する

```text
public/character/
├── base.png          # 1枚絵
├── happy.png         # 表情差分（後で追加）
├── troubled.png
└── working/          # 連番アニメ（さらに後で）
    ├── 01.png
    └── 02.png
```

### 7.3 アニメーション制御の窓口

フロントは状態名だけを受け取る。フレームや秒数の管理はアニメーションモジュールに閉じ込める。

```ts
character.setState("working");
```

---

## 8. Phase 4 ― Secretary Core（Rust）と fake event

**2026-09-03 完了。** 実装は `src-tauri/src/server.rs`（axumのHTTPサーバーと、Trackerを包む `Core`）、`src-tauri/src/config.rs`（`config.toml` と認証トークン）、`scripts/fake-turn.sh`。設計からの変更点:

- `/debug/event` は作らず、`scripts/fake-turn.sh` が本物と同じ `/hook` へトークン付きで投げる。経路が一つで済む。
- 一時状態の期限切れは、500ミリ秒ごとにスナップショットを計算し「前回配信と異なるときだけ」配信する定期処理で反映する。同じ処理が2分ごとに失効セッションも掃除する。
- 追跡方針はトレイの「すべてのセッションを追跡」で実行中に切り替えられる。既定は設定ファイルの `follow`。
- 設定ファイルとトークンは `~/Library/Application Support/com.nagatadaichi.tauriapp/` に置く。hookスクリプトは同じトークンファイルを読む。

以下は着手前の計画。

Claude Codeとの接続前に、Rust側の中核を作り、偽のイベントで一連の流れを通す。

### 8.1 構成

状態機械、セッション追跡、スナップショット生成はPhase 1で `crates/secretary-core` に実装済み。Phase 4で作るのは、それを包むHTTPサーバーとTauriへの配線、fake event、設定ファイルである。`src-tauri/src/` は次のように分ける。

```text
src-tauri/src/
├── main.rs
├── lib.rs           # Tauriのセットアップ、trayとwindow
├── server.rs        # axumによる HTTP サーバー（/hook, /health, /debug/event）
├── bridge.rs        # secretary-core の Tracker を Mutex で保持し、スナップショットを Tauri event で送る
└── config.rs        # 設定ファイル（follow 方針、ポート、保持秒数）

crates/secretary-core/src/   # Phase 1 で実装済み
├── hook.rs          # hook JSON → HookEvent、Discord タグの判定
├── classify.rs      # ツール分類（Bash の読み取り判定を含む）
├── session.rs       # セッション単位の状態機械
├── tracker.rs       # 複数セッションの束ね、追跡方針、失効
├── snapshot.rs      # UI へ渡す SecretarySnapshot（ts-rs で TS 型を生成）
└── state.rs         # AssistantState と優先度
```

### 8.2 HTTPサーバー

- `axum` を使い、`127.0.0.1` のみに束縛する。ポートは設定可能で既定は `47831`。
- `POST /hook` はhook JSONを受け取り、即座に `204` を返す。処理は非同期で行い、hookをブロックしない。
- `GET /health` は稼働確認用。
- 起動時に乱数トークンをアプリのデータディレクトリに書き出し、`Authorization: Bearer` で検査する。hookクライアントはそのファイルを読む。他のローカルプロセスによる偽イベントを防ぐ目的で、コストは小さい。

### 8.3 UIへの通知

状態が変わるたびに、Rust側がスナップショットをTauri eventで送る。フロントは差分計算をせず、受け取ったものを描く。

```rust
#[derive(Serialize, TS)]
#[ts(export)]
pub struct SecretarySnapshot {
    pub status: AssistantState,
    pub message: Option<String>,        // 吹き出し
    pub current_tool: Option<String>,   // ステータスパネル用
    pub task_summary: Option<String>,   // 直近のプロンプト要約
    pub pending_permission: Option<String>,
    pub session_label: Option<String>,
}
```

### 8.4 fake event

開発用に、状態遷移をコマンドやショートカットから起こせるようにする。

- `POST /debug/event` で任意のhook JSONを投げられる（デバッグビルドのみ有効）
- 開発用スクリプト `scripts/fake-turn.sh` が「プロンプト → Bash → Edit → reply → Stop」の一連を順に投げる

### 8.5 テスト

- 状態機械と並列ツールのカウンタは `cargo test` でユニットテストする
- 一時状態のタイマーは、時計を注入して決定的にテストする

### 8.6 完了条件

fake eventを投げると、キャラクターが idle → thinking → working → success → idle と動き、吹き出しにメッセージが出る。

---

## 9. Phase 5 ― Claude Codeとの接続

**2026-09-03 判断: 後回し。** 接続そのものはPhase 4で完了しており、シェルスクリプトのhookクライアントで実運用できている。Rust製CLIへの置き換えはWindows対応のためのものだったが、Windows対応は行わない方針になったため、必要が生じるまで着手しない。

### 9.1 hookクライアントをRustで作る（保留）

Phase 0のシェルスクリプトを、Cargoワークスペース内の小さなバイナリ `secretary-hook` に置き換える。

- stdinを読み、トークンを付けてPOSTし、必ず終了コード0で終わる。タイムアウトは1秒
- サーバーが起動していなければ黙って終了する。Claude Codeの動作を決して妨げない
- 単一バイナリなのでWindows対応時にもそのまま使える

```text
Cargo workspace
├── src-tauri/            # アプリ本体
└── crates/
    └── secretary-hook/   # hookクライアント CLI
```

### 9.2 接続の完了条件

Discordから指示を出したとき、デスクトップの秘書が次のように反応する。

```text
Discord: 「lifeのテストを実行して」
   ↓
秘書: thinking（考える動き）
   ↓
秘書: working（Bash実行中。パネルに「bun test」）
   ↓
権限が必要なら: waiting（吹き出し「Bashの実行許可を待っています」）
   ↓
Claudeが reply ツールで返信 → 吹き出しに返信本文の冒頭
   ↓
Stop → success（跳ねる）→ idle
```

---

## 10. Phase 6 ― 吹き出し

**2026-09-03 完了。** 実装は `src/bubble/bubble.ts` と `src/styles.css`。文言と種別（reply / assistant / permission / system）はRust側のスナップショットに乗っているので、フロントは種別ごとの見た目と表示時間を決めるだけ。表示時間は文字数に応じて4〜12秒、system は6秒、permission は状態が変わるまで出しっぱなし。同じ文言のスナップショットが続いても再表示しない。吹き出しの余白のためウィンドウを280×420に広げ、キャラクターは下部240×320に固定した。

課題: ウィンドウが広がったぶん、透明な余白もクリックを受け止めて背後のアプリを触れなくなる。Phase 8でカーソル位置に応じてクリック透過を自動で切り替える対応を検討する。

キャラクターを「秘書」に見せる中核UI。

### 10.1 文言の出所

優先度順。

1. **Discordへの返信本文。** `mcp__plugin_discord_discord__reply` の `tool_input.text` を先頭80文字程度に切って表示する。Discordに送った言葉と秘書の言葉が一致するので、自然に感じられる（実機で取得を確認済み）
2. **ターン終了時の最終応答。** Stop の `last_assistant_message` を同様に切って表示する。Discordを介さないセッションでもClaudeの言葉を話せる。ただしDiscord由来のターンでは「Discordへの返信を送信しました…」のような作業報告になるため、1が取れたターンでは使わない（観測済み）
3. **権限要求。** 「`{tool_name}` の実行許可を待っています」
4. **状態テンプレート。** 上の3つが無いとき、状態ごとの定型文から選ぶ（Phase 9で拡充）

表示する文言はローカル表示専用で、実行や送信には使わない。Discordの外部ユーザーが書いた文字列が `prompt` 経由で届くことがあるため、それを命令として解釈する処理は一切置かない。

### 10.2 コンポーネント

```text
SpeechBubble
├── text
├── kind        # speech | system | permission
├── duration_ms # 0 なら状態が変わるまで表示
└── priority    # permission > speech > system
```

---

## 11. Phase 7 ― ステータスパネル

**2026-09-03 完了。** 実装は `src/panel/panel.ts`。吹き出しが頭上を使うので、パネルは足元に半透明の字幕風カードとして置いた。追跡セッションが無ければ非表示、thinking / working / waiting / success / error で表示、idle に戻って4秒後に畳む。3行目は許可待ちのツール名を優先し、無ければ実行中ツール。状態の点は色と点滅で状態を示す。

必要なときだけ小さなパネルを展開する。

```text
┌────────────────────────────┐
│ Claude Code    life        │
│ ● Working                  │
│                            │
│ Task                       │
│ lifeのテストを実行して       │
│                            │
│ Current                    │
│ Bash: bun test             │
└────────────────────────────┘
```

- 表示条件: workingまたはwaitingのとき自動展開。idleで数秒後に折りたたむ
- 内容: 追跡中セッションのラベル（cwdの末尾）、状態、直近プロンプトの要約、実行中ツール
- 直近プロンプトの要約は、Rust側で先頭行を切り出すだけにする

---

## 12. Phase 8 ― インタラクションと常駐メニュー

**2026-09-03 完了。** 実装は `src-tauri/src/lib.rs`（`cursor_watch`、`show_context_menu`、トレイの状態行）と `src/panel/panel.ts`（固定表示）。

- **クリック透過の自動化。** Rust側が60ミリ秒ごとにカーソル位置を見て、キャラクターの矩形（余白16px込み）の外にあるときだけ `set_ignore_cursor_events(true)` にする。吹き出しやパネルの余白は常にすり抜ける。トレイの「クリック透過(常時)」は手動の上書き。
- **右クリックメニュー。** WebViewの `contextmenu` からRustのコマンドを呼び、ネイティブのポップアップメニューを出す。項目は「今のタスクを表示」（パネルの固定）と「設定ファイルを開く」。左クリックはドラッグ開始に使われるため、メニューは右クリックに限定した。
- **トレイの状態行。** 先頭に「<セッション> · <状態>」を表示し、スナップショット配信のたびに更新する。「設定ファイルを開く」もトレイに追加。
- 計画にあった「話しかける」「ログを見る」は入れていない。前者はPhase 10、後者は必要になったら別ウィンドウで。

### 12.0 透明な余白のクリック透過（Phase 6 で顕在化）

ウィンドウの透明な部分もクリックを受け止めてしまう。Rust側で定期的にカーソル位置を取り、キャラクターの矩形の外にあるときは `set_ignore_cursor_events(true)`、中に入ったら false に戻す方式を検討する。トレイの「クリック透過」は手動の上書きとして残す。（上記のとおり実装済み）

### 12.1 キャラクターのクリック

```text
        🧍
         ↓
 ┌──────────────────┐
 │ 📋 今のタスク     │
 │ 📜 最近のイベント │
 │ 👁 クリック透過   │
 │ ⚙ 設定            │
 └──────────────────┘
```

「話しかける」はこのフェーズには含めない。Phase 10で扱う。

### 12.2 メニューバーのトレイ

透明ウィンドウ上の右クリックは誤操作しやすいので、終了や設定はメニューバーのアイコンに置く。

```text
追跡中: life
────────────
クリック透過
常に前面
設定…
────────────
終了
```

---

## 13. Phase 9 ― キャラクターの人格

**2026-09-03 完了。** 実装は `src-tauri/src/persona.rs`。辞書は `~/Library/Application Support/com.nagatadaichi.tauriapp/persona.toml` に初回起動時に書き出され、編集して再起動すれば差し替わる。規則:

- Claudeの実際の言葉（Discordへの返信本文、最終応答）があるときは触らない。文言が無いときだけ定型文で埋める。
- 指示を受け取った直後は `turn_start`、作業（thinking / working）が `long_work_after_secs`（既定20秒）続いたらターンにつき一度だけ `long_work`、失敗なく終わってClaudeの言葉が無ければ `success`。
- 許可待ちの文言は `waiting` で言い換え、`{tool}` にツール名が入る。失敗の文言は `error` の一言を先頭に添える。
- 各項目は候補の配列で、直前と同じ文言は避けて選ぶ。

状態テンプレートを辞書として持ち、Rust側で選ぶ。

| 状態 | 例 |
|---|---|
| ターン開始 | 「はい、お任せください！」 |
| 長い作業 | 「少々お待ちください……」 |
| 成功 | 「完了しました！」 |
| 失敗 | 「……すみません、問題が見つかりました。」 |
| 権限待ち | 「こちらは確認が必要です。Discordで承認をお願いします。」 |

- 辞書はTOMLファイルにして、アプリを再ビルドせずに差し替えられるようにする
- 同じ状態が続くときは同じ文言を繰り返さない
- 「長い作業」はworkingが一定秒数続いたときに発火する

---

## 14. Phase 10 ― 秘書からClaude Codeへの指示

**2026-09-03 完了。** 候補1（秘書アプリ自身をClaude Codeのchannelにする）で実装した。

### 14.1 調査で分かったこと

- Claude Codeのchannelは「`capabilities.experimental["claude/channel"]` を宣言したstdio MCPサーバー」。サーバーが `notifications/claude/channel`（`content` と `meta`）を送ると、セッションに `<channel source="<サーバー名>" …>本文</channel>` のターンとして届く（`meta` の各キーがタグの属性になる）。Claudeが busy のときは次のターンまで溜まる。
- `claude/channel/permission` も宣言すると、権限要求が `notifications/claude/channel/permission_request`（`request_id`, `tool_name`, `description`, `input_preview`）で中継され、`notifications/claude/channel/permission`（`request_id`, `behavior: allow|deny`）で答えられる。端末側のダイアログと並走し、先に届いた答えが採用される。
- 研究プレビュー中は `--channels` にAnthropicの許可リスト上のプラグインしか渡せない。自作channelは `claude --dangerously-load-development-channels server:secretary` で読み込む（起動時に確認ダイアログが出る。`--help` には出ない）。
- MCPサーバーの子プロセスには `CLAUDE_CODE_SESSION_ID` と `CLAUDE_PROJECT_DIR` が環境変数で渡される（実機で確認）。これでhookのsession_idとそのまま突き合わせられる。
- Discordへの自動投稿（候補2）は採用しない。

### 14.2 構成

```text
 秘書アプリ ──ChannelCommand(SendPrompt / RespondPermission)──▶ secretary-channel ──MCP notification──▶ Claude Code
 秘書アプリ ◀──ChannelEvent(Hello / Reply / PermissionRequest)── secretary-channel ◀──MCP request/notif── Claude Code
```

- `crates/secretary-channel`: Rust製のstdio MCPサーバー（依存はserdeのみ、MCPは手書き）。`scripts/install-channel.sh` でユーザースコープのMCPサーバー `secretary` として登録する。Claude Codeがセッションごとに起動し、アプリとは `~/Library/Application Support/com.nagatadaichi.tauriapp/channel.sock`（Unixソケット、0600、hookと同じトークンで認証）で1行1JSONを交換する。アプリが無ければ2秒ごとに再接続を試すだけで、MCPとしては普通に応答する。
- 型は `secretary-core::channel`（`ChannelEvent` / `ChannelCommand` / `RelayedPermission`）。計画にあった `SecretaryCommand` はこの2つに置き換えた。
- アプリ側は `src-tauri/src/channel.rs`（接続台帳とソケット）、`server::Core::send_prompt` / `respond_permission`。送り先は表示中のセッション、接続が無ければ最新の接続。`POST /say`、`POST /permission` でスクリプトからも送れる（`scripts/say.sh`, `scripts/permission.sh`）。
- 追跡: `<channel source="secretary">` 由来のプロンプトと、秘書channelから中継された権限要求はDiscordと同じ「channel由来」として扱う（`FollowPolicy::Channel`、設定値は従来どおり `"discord"`）。`mcp__secretary__reply` はDiscordの返信ツールと同じReplyクラス。
- UI: キャラクターに触れると出る💬ボタン、右クリックとトレイの「話しかける…」で入力欄を開く（Enterで送信、Escで閉じる）。中継された権限要求は吹き出しに説明と引数のプレビュー、「許可 / 拒否」ボタンを出す。入力欄やボタンが出ている間はウィンドウ全体でクリックを受ける。文言は `persona.toml` の `waiting_relayed`（`{tool}` は `secretary: reply` のように短くしたツール名）。

### 14.3 実機確認（2026-09-03）

`claude --dangerously-load-development-channels server:secretary --permission-mode default` で起動したセッションに対して:

1. `scripts/say.sh "1たす1は？短く答えて"` → 端末に `← secretary: 1たす1は？短く答えて` と表示され、hookには `<channel source="secretary" chat_id="desktop" ts="…">` のUserPromptSubmitが届いた。
2. Claudeが `reply` ツールを呼び、その実行許可が `permission_request`（id=xakdy）として秘書に中継された。吹き出しは「「secretary: reply」を実行してよいですか？下のボタンで答えてください。」。
3. `scripts/permission.sh xakdy allow` → ツールが実行され、「2です。」が吹き出しに出て、Stopまで到達した。
4. 別のターンで `deny` → Claudeは拒否を受けて文章で答え、Stopで片付いた（PermissionDeniedは届かなかった）。

### 14.4 制約

- 開発用フラグはセッションごとに指定が要る。`--channels` で使えるのはAnthropicの許可リストに載ってから。
- MCPサーバーはユーザースコープなので、全セッションで `secretary-channel` が起動し `reply` ツールが見える（説明文で「秘書からのメッセージにだけ使う」と指示している）。外すときは `claude mcp remove -s user secretary`。
- `reply` ツールの実行にも権限確認が出る設定では毎回中継される。煩わしければ `permissions.allow` に `mcp__secretary__reply` を足す。

---

## 15. Phase 11 ― 常駐、仕上げ

**2026-09-03 判断: Windows対応は行わない。** 以下のWindows関連の項目は参考として残すが、着手しない。

- ログイン時の自動起動（`tauri-plugin-autostart`）
- 通知（`tauri-plugin-notification`）。waitingへ入ったときに通知を出す
- 設定画面（追跡対象cwd、ポート、表示サイズ、文言辞書のパス）
- Windows対応。透明ウィンドウは `macos-private-api` 不要。装飾なしウィンドウでは `shadow: false` が必須。トレイはそのまま使える。`secretary-hook` はRust製なのでそのまま動く。実機での検証を必ず行う
- アニメーションと素材のブラッシュアップ

---

## 16. イベントプロトコル

### 16.1 入力: Claude Code hook JSON

hookから届くJSONをそのまま受け取り、Rust側で型に落とす。フィールドはPhase 0の記録で確定させる。

```rust
#[derive(Deserialize)]
pub struct HookEnvelope {
    pub session_id: String,
    pub hook_event_name: String,
    pub cwd: Option<String>,
    pub transcript_path: Option<String>,
    pub prompt_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_use_id: Option<String>,
    pub tool_input: Option<serde_json::Value>,
    pub prompt: Option<String>,
    pub last_assistant_message: Option<String>,
    pub notification_type: Option<String>,
    pub error: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
```

未知のフィールドは `extra` に保持し、ログに残す。

### 16.2 出力: UIへ渡す型

```rust
#[derive(Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum SecretaryEvent {
    Snapshot(SecretarySnapshot),
    Speech { text: String, kind: SpeechKind, duration_ms: u32 },
    TurnStarted { session_id: String, summary: String },
    TurnFinished { session_id: String, outcome: TurnOutcome },
    PermissionRequested { session_id: String, tool_name: String },
}
```

`ts-rs` で `src/generated/` にTypeScript型を書き出し、フロントはそれだけを参照する。

### 16.3 境界の原則

- UIはClaude Codeのhook形式を知らない。知るのはSecretaryEventだけ
- Secretary Coreはフロントの都合を知らない。描画に関する判断は持たない
- この境界を守れば、将来Webダッシュボードや通知だけのクライアントを同じCoreに接続できる

---

## 17. リポジトリ構成

現在の雛形をそのまま使う。モノレポにはしない。

```text
tauri-app/
├── src/                      # フロントエンド（TypeScript）
│   ├── main.ts
│   ├── character/            # 表示と動き
│   ├── bubble/               # 吹き出し
│   ├── panel/                # ステータスパネル
│   └── generated/            # ts-rs が生成する型（コミットする）
├── src-tauri/                # アプリ本体（Rust）
│   └── src/                  # 8.1 の構成
├── crates/
│   ├── secretary-core/       # 状態機械（Phase 1 で実装済み）
│   ├── hook-logger/          # Phase 0 のログサーバー
│   └── secretary-hook/       # hookクライアント CLI（Phase 5 から）
├── public/
│   └── character/            # base.png を置く。無ければ placeholder.svg
├── hooks/
│   └── secretary-hook.sh     # Phase 0 用。インストール手順を README に書く
├── scripts/
│   └── fake-turn.sh
├── docs/
│   ├── plan.md               # この文書
│   ├── spike-results.md      # Phase 0 の記録
│   └── protocol.md           # Phase 4 以降に整備
└── README.md
```

`crates/` を追加した時点でルートに `Cargo.toml` のワークスペース定義を置く。

---

## 18. 開発フェーズ一覧

| Phase | 内容 | ゴール |
|---|---|---|
| 0 | イベント源の検証スパイク | 6状態を導出できるシグナルの確定（完了） |
| 1 | 状態と遷移の設計 | 対応表と遷移規則の確定（完了。`docs/state-machine.md` と `crates/secretary-core`） |
| 2 | Tauri最小アプリ | 透明なデスクトップキャラ（macOS）（完了） |
| 3 | キャラクター表示と動き | 1枚絵が状態ごとに動く（完了） |
| 4 | Secretary Core の配線と fake event | HTTP サーバーと Tauri への接続。偽イベントで一連の遷移が動く（完了） |
| 5 | Claude Code接続 | 実際の作業に秘書が反応する（接続はPhase 4で完了。Rust製CLIは保留） |
| 6 | 吹き出し | Claudeの発言が秘書の言葉になる |
| 7 | ステータスパネル | タスクを可視化（完了） |
| 8 | インタラクションと常駐メニュー | キャラを操作できる（完了） |
| 9 | 人格 | 秘書らしくなる（完了） |
| 10 | 秘書からの指示（後続） | 逆方向の経路 |
| 11 | 常駐・仕上げ | 常用できるアプリ（Windows対応は行わない） |

### MVPの定義

- **MVP-1（Phase 0〜4）:** fake eventでキャラクターが生きて見える
- **MVP-2（Phase 5〜6）:** 実際のClaude Codeの作業に反応し、Claudeの言葉を吹き出しで話す

MVP-2に到達した時点で、冒頭の最終コンセプトが最小構成で実現する。

---

## 19. 開発優先順位

最優先:

1. Phase 0 のスパイク。イベント源が取れなければ計画全体を見直す
2. Tauri透明ウィンドウ（macOS）
3. 1枚絵の表示と手続き的な動き
4. Rust側の状態機械とユニットテスト
5. fake eventによる状態遷移
6. hook接続（実イベント）
7. 吹き出しにClaudeの返信をミラー

その後:

8. ステータスパネル
9. クリック操作とトレイメニュー
10. 人格テンプレート
11. 通知
12. 設定画面
13. 自動起動
14. 逆方向の指示（channel化の調査から）
15. 素材とアニメーションのブラッシュアップ

---

## 20. リスクと未確認事項

| 項目 | 影響 | 対処 |
|---|---|---|
| auto modeでは権限確認がほぼ出ず、waiting状態を検証しにくい | waitingの表示検証が遅れる | `--permission-mode default` の検証セッションで確認する。PermissionDeniedも購読済み |
| SessionEndが届かないセッションがある（観測済み） | 追跡対象が幽霊化する | 一定時間イベントの無いセッションを失効させる |
| ClaudeがReadではなくBashで読み取ることがある（観測済み） | 読み取りがworkingと表示される | Bashのコマンド先頭語で読み取り系を判定する |
| 権限拒否時に PostToolUse が来ず、実行中集合が漏れる（確認済み） | 状態が working のまま残る | Stop で集合を空にする。auto mode では PermissionDenied でも除去する。手動Deny時の挙動は Discord での観測で確認 |
| `macos-private-api` の使用 | App Store配布不可 | 個人利用のため許容 |
| Claude Code側のhook仕様変更 | 接続が壊れる | `secretary-hook` は未知フィールドを保持し、Coreは欠損に寛容にする |
| 素材制作の工数 | UIの完成度 | 1枚絵と手続き的な動きで先に体験を作る |
| Phase 10のchannel化は仕様が公開情報として十分でない | 逆方向機能の実現性 | 2026-09-03 解消。公式のChannels referenceと実機で確認し、実装済み（14章） |

---

## 21. 設計方針

### 21.1 UIとClaude Codeを直接結合しない

```text
Claude Code ──hooks──▶ Secretary Core ──event──▶ UI
```

UIはClaude Codeの内部を知らない。Secretary Coreはhookの形式を吸収する唯一の層で、ここだけを直せば仕様変更に追随できる。

### 21.2 Rustが真実を持つ

状態、メッセージ、追跡対象の判断はすべてRust側で行い、テストで守る。フロントは受け取ったスナップショットを描くだけにして、ロジックの二重化を避ける。

### 21.3 リスクの高いものを先に検証する

見た目の作り込みは楽しいが、イベント源が取れなければすべて無駄になる。Phase 0を半日で終わらせ、その結果で設計を固めてから作り始める。

### 21.4 Claude Codeの動作を決して妨げない

hookクライアントは常に即座に終了する。サーバーが落ちていても、トークンが無くても、Claude Codeは通常どおり動く。秘書は「見守る」存在であって、作業の経路上に立たない。
