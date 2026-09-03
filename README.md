# Claude Code デスクトップ秘書

Claude Code の作業状況を、デスクトップに常駐するキャラクターとして可視化する Tauri + Rust アプリ。

- 企画書: `docs/plan.md`
- 状態機械の仕様: `docs/state-machine.md`
- hook イベントの観測記録: `docs/spike-results.md`

## 必要なもの

- macOS(現時点の対象)、Rust stable、Bun、Xcode Command Line Tools
- Tauri CLI(`cargo install tauri-cli --version "^2"` または `bun run tauri`)

## 起動

```sh
bun install
bun run tauri dev
```

Dock にはアイコンが出ず、メニューバーにアイコンが出る。透明なウィンドウにキャラクターだけが表示され、ドラッグで動かせる。位置は終了時に保存され、次回同じ場所に出る。Claude Code が Discord へ返信すると、その本文が頭上の吹き出しに出る。作業中は足元に小さなパネルが開き、状態、セッション名、直近の依頼、実行中のツールを表示する。

キャラクターの外側の透明な部分は自動でクリックがすり抜けるので、背後のアプリをそのまま操作できる。キャラクターの上ではドラッグと右クリックが効く。右クリックメニューから、足元のパネルの固定表示と設定ファイルを開く操作ができる。

メニューバーのアイコンには追跡中のセッションと状態が表示され、「クリック透過(常時)」「常に前面に表示」「すべてのセッションを追跡」の切り替え、「設定ファイルを開く」「終了」ができる。

開発ビルドではメニューに「デバッグ」が加わり、6 状態(idle / thinking / working / waiting / success / error)を手で切り替えたり、一巡のデモを再生したりできる。起動時に環境変数 `SECRETARY_DEMO=1` を付けると 3 秒ごとに状態を巡回し続ける。

```sh
SECRETARY_DEMO=1 bun run tauri dev
```

## 秘書の口調

`persona.toml` を編集すると吹き出しの定型文を差し替えられる。指示を受けたとき(`turn_start`)、作業が長引いたとき(`long_work`)、成功(`success`)、失敗の前置き(`error`)、許可待ちの言い換え(`waiting`、`{tool}` がツール名になる)の 5 種類で、各項目は候補の配列。Claude が Discord に返した本文や最終応答があるときはそちらが優先され、定型文は文言が無いときだけ使われる。

## キャラクター画像

`public/character/base.png` に透過 PNG を置くと、その画像が表示される。推奨サイズは 240×320 程度(ウィンドウと同じ比率)。無い場合は `public/character/placeholder.svg` の仮キャラクターが出る。

状態ごとの差分画像は任意。`public/character/<state>.png`(例: `success.png`、`error.png`)を置くと、その状態のときだけ差し替わる。動き(呼吸、首かしげ、跳躍、震え)とバッジは 1 枚絵のままでも付く。

## テスト

```sh
cargo test --workspace          # 状態機械(実ログの再生を含む)、設定、サーバー
bun test                        # フロントエンド(吹き出しの表示時間など)
```

## Claude Code との接続

```sh
./scripts/install-hooks.sh     # ~/.claude/settings.json に hook を登録(バックアップ付き、再実行可)
bun run tauri dev              # アプリが 127.0.0.1:47831 で hook を受け付ける
```

アプリは起動時に `~/Library/Application Support/com.nagatadaichi.tauriapp/` へ `config.toml`(設定)、`persona.toml`(秘書の口調の辞書)、`token`(hook の認証トークン)を書く。hook スクリプトは同じトークンを読んで `Authorization: Bearer` で送る。設定を変えたらアプリを再起動する。

既定では Discord 由来のセッションだけを表示する。ターミナルで動かしている Claude Code にも反応させたいときは、メニューバーの「すべてのセッションを追跡」をオンにするか、`config.toml` の `follow` を `"all"` にする。

偽のイベントで一連の状態遷移を確かめるには次を実行する。

```sh
./scripts/fake-turn.sh          # thinking → working → waiting → success
./scripts/fake-turn.sh --fail   # 途中でコマンドが失敗するターン
```

Phase 0 のログサーバー `cargo run -p hook-logger` は同じポートを使うので、アプリと同時には起動しない。

### 秘書から話しかける(Phase 10)

秘書アプリ自身を Claude Code の channel にする。`secretary-channel`(stdio MCP サーバー)を Claude Code がセッションごとに起動し、アプリとは Unix ソケット `channel.sock` でつながる。

```sh
./scripts/install-channel.sh   # secretary-channel をビルドし、ユーザースコープの MCP サーバー "secretary" として登録(再実行可)
claude --dangerously-load-development-channels server:secretary   # channel を有効にしてセッションを起動
```

研究プレビュー中は自作 channel を `--channels` に渡せないため、開発用フラグで読み込む。起動時の確認で「I am using this for local development」を選ぶ。

キャラクターに触れると出る 💬、右クリックかトレイの「話しかける…」で入力欄が開く。Enter で送ると、そのセッションに `<channel source="secretary">` のターンとして届き、Claude は `reply` ツールで吹き出しに短く返事をする。送り先は表示中のセッション、無ければ最新の接続。権限確認が出たとき(`--permission-mode default` など)は吹き出しに内容と「許可 / 拒否」が出て、そこから答えられる。文言は `persona.toml` の `waiting_relayed`。

ターミナルからも送れる。

```sh
./scripts/say.sh "テストを走らせて"            # POST /say
./scripts/permission.sh <request_id> allow     # POST /permission。request_id はログの [channel] permission_request に出る
```

登録を外すには `claude mcp remove -s user secretary`。

## 構成

```text
src/                    フロントエンド(TypeScript)。src/generated/ は Rust から生成した型
src-tauri/              アプリ本体(Rust)
crates/secretary-core/  状態機械と channel の型。Tauri に依存しない
crates/secretary-channel/ 秘書を Claude Code の channel にする stdio MCP サーバー(Phase 10)
crates/hook-logger/     Phase 0 用のログサーバー
public/character/       キャラクター画像
docs/                   企画書と仕様
```
