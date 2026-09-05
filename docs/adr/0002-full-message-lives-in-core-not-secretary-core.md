# 発言の全文は `secretary-core` ではなく `src-tauri` の `Core` に持つ

全文ビューが読む「切られていない本文」を、状態機械を持つ `secretary-core`（`Tracker` / `SessionState`）ではなく、アプリ側の `Core`（`src-tauri/src/server.rs`）に 1 件だけ保持する。素直に考えれば発言はセッションの持ち物なので `SessionState` に置きたくなるが、そうしない理由が 3 つある。

## 理由

- **セッションと寿命が違う**: `Tracker::apply` は `session.ended()` になったセッションを即座に `HashMap` から削除し、`expire_stale` も 30 分無音で消す。`SessionState` に置くと、Claude Code を終了した瞬間に全文が消える。長い応答を読み返したいのはまさにその直後で、全文ビューの存在意義と噛み合わない
- **`secretary-core` は壁時計を持たない設計契約がある**: crate ドキュメントに「時刻は呼び出し側から `Instant` で渡すので、テストでは時間を自由に進められる」と明記されている。全文ビューは発言の絶対時刻を表示するため、ここに置くと壁時計を持ち込んで契約を破ることになる
- **全文は状態導出に一切関与しない**: キャラクターの状態も吹き出しの文言も、切られた側の文字列から決まる。全文は表示のための便宜的な控えでしかなく、状態機械の関心事ではない

## Consequences

- 実装上の追加はほぼ無い。`Core::ingest` は既に `Tracker::apply` の戻り値 `HookEvent` を受け取っていて、`HookEvent::Stop { last_assistant_message }` が未切断の本文をそのまま持っている。`secretary-core` に手を入れずに捕まえられる
- 保持するのは追跡対象のセッションのぶんだけ。`Core` は全セッションの `Stop` を見てしまうので、`Tracker` に追跡対象かどうかを問い合わせる必要がある
- markdown 記号を削ぐ処理だけは `secretary-core` に公開ヘルパーとして置き、吹き出し側（`set_speech`）と全文側（`Core`）の両方から呼ぶ。経路が 2 つに分かれたため、片方だけ削ぐと表示が食い違う。純粋な文字列関数なので上記の設計契約は汚さない
