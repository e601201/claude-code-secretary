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

Dock にはアイコンが出ず、メニューバーにアイコンが出る。透明なウィンドウにキャラクターだけが表示され、ドラッグで動かせる。位置は終了時に保存され、次回同じ場所に出る。

メニューバーのアイコンから「クリック透過」「常に前面に表示」の切り替えと「終了」ができる。

## キャラクター画像

`public/character/base.png` に透過 PNG を置くと、その画像が表示される。推奨サイズは 240×320 程度(ウィンドウと同じ比率)。無い場合は `public/character/placeholder.svg` の仮キャラクターが出る。

## テスト

```sh
cargo test -p secretary-core   # 状態機械のユニットテストと実ログの再生テスト
```

## Claude Code との接続(Phase 0 の検証用)

```sh
cargo run -p hook-logger       # hook を受けてログに残すだけのサーバー
./scripts/install-hooks.sh     # ~/.claude/settings.json に hook を登録(バックアップ付き)
```

## 構成

```text
src/                    フロントエンド(TypeScript)。src/generated/ は Rust から生成した型
src-tauri/              アプリ本体(Rust)
crates/secretary-core/  状態機械。Tauri に依存しない
crates/hook-logger/     Phase 0 用のログサーバー
public/character/       キャラクター画像
docs/                   企画書と仕様
```
