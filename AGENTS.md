# Codex デスクトップ秘書

Codex の作業状況をデスクトップ常駐キャラクターとして可視化する Tauri + Rust アプリ。
Cargo workspace(`src-tauri` + `crates/{hook-logger,secretary-channel,secretary-core}`)と Vite/TypeScript のフロントエンドで構成。

- 企画書: `docs/plan.md`
- 状態機械の仕様: `docs/state-machine.md`

## Agent skills

### Issue tracker

Issue は GitHub Issues(`e601201/claude-code-secretary`)で管理し、`gh` CLI で操作する。See `docs/agents/issue-tracker.md`.

### Triage labels

5 つの正準ロールをラベル名そのままで使う(`needs-triage` / `needs-info` / `ready-for-agent` / `ready-for-human` / `wontfix`)。See `docs/agents/triage-labels.md`.

### Domain docs

single-context: ルートの `CONTEXT.md` と `docs/adr/`。See `docs/agents/domain.md`.
