//! secretary-core のスナップショットを webview へ届ける橋渡し。
//!
//! Rust 側が唯一の真実を持ち、フロントエンドは受け取った [`SecretarySnapshot`] を描くだけ。
//! Phase 4 で Tracker がここへスナップショットを流し込む。Phase 3 ではデモ用の生成のみ。

use std::{sync::Mutex, thread, time::Duration};

use secretary_core::{AssistantState, RelayedPermission, SecretarySnapshot, SpeechKind};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::CHARACTER_WINDOW;

/// webview が購読するイベント名。
pub const SNAPSHOT_EVENT: &str = "secretary://snapshot";

/// 6 状態を表示順に並べたもの。デモやデバッグメニューで使う。
pub const ALL_STATES: [AssistantState; 6] = [
    AssistantState::Idle,
    AssistantState::Thinking,
    AssistantState::Working,
    AssistantState::Waiting,
    AssistantState::Success,
    AssistantState::Error,
];

/// 最後に配信したスナップショット。起動直後の webview が取りに来る。
pub struct SnapshotBridge {
    last: Mutex<SecretarySnapshot>,
}

impl SnapshotBridge {
    pub fn new() -> Self {
        Self {
            last: Mutex::new(SecretarySnapshot::idle()),
        }
    }

    pub fn last(&self) -> SecretarySnapshot {
        self.last.lock().unwrap().clone()
    }
}

/// スナップショットを保持し、キャラクターウィンドウへ送る。
pub fn publish<R: Runtime>(app: &AppHandle<R>, snapshot: SecretarySnapshot) {
    if let Some(bridge) = app.try_state::<SnapshotBridge>() {
        *bridge.last.lock().unwrap() = snapshot.clone();
    }
    crate::update_tray_status(app, &snapshot);
    if let Err(e) = app.emit_to(CHARACTER_WINDOW, SNAPSHOT_EVENT, &snapshot) {
        eprintln!("emit snapshot failed: {e}");
    }
}

#[tauri::command]
pub fn get_snapshot(bridge: tauri::State<'_, SnapshotBridge>) -> SecretarySnapshot {
    bridge.last()
}

/// 状態名(snake_case)から状態を引く。デバッグメニューの id に使う。
pub fn state_from_name(name: &str) -> Option<AssistantState> {
    ALL_STATES.into_iter().find(|s| state_name(*s) == name)
}

pub fn state_name(state: AssistantState) -> &'static str {
    match state {
        AssistantState::Idle => "idle",
        AssistantState::Thinking => "thinking",
        AssistantState::Working => "working",
        AssistantState::Waiting => "waiting",
        AssistantState::Success => "success",
        AssistantState::Error => "error",
    }
}

/// デモ用のスナップショット。文言は Phase 9 のテンプレートの仮置き。
pub fn demo_snapshot(state: AssistantState) -> SecretarySnapshot {
    let (message, kind, tool) = match state {
        AssistantState::Idle => (None, None, None),
        AssistantState::Thinking => (Some("考えています……"), Some(SpeechKind::Assistant), None),
        AssistantState::Working => (
            Some("コードを修正しています……"),
            Some(SpeechKind::Assistant),
            Some("Edit: src/main.ts"),
        ),
        AssistantState::Waiting => (
            Some("Bash の実行許可を待っています"),
            Some(SpeechKind::Permission),
            Some("Bash: bun test"),
        ),
        AssistantState::Success => (Some("完了しました！"), Some(SpeechKind::Reply), None),
        AssistantState::Error => (
            Some("Bash が失敗しました: Exit code 1"),
            Some(SpeechKind::System),
            None,
        ),
    };
    SecretarySnapshot {
        status: state,
        message: message.map(str::to_string),
        message_kind: kind,
        current_tool: tool.map(str::to_string),
        task_summary: Some("デモ: 状態を順に表示".to_string()),
        pending_permission: (state == AssistantState::Waiting).then(|| "Bash".to_string()),
        // 許可 / 拒否ボタンの見た目を確認できるように、中継された権限要求も付ける
        relayed_permission: (state == AssistantState::Waiting).then(|| RelayedPermission {
            request_id: "demo0".to_string(),
            tool_name: "Bash".to_string(),
            description: "テストを実行する".to_string(),
            input_preview: "{\"command\":\"bun test\"}".to_string(),
        }),
        session_id: Some("demo".to_string()),
        session_label: Some("demo".to_string()),
        tracked_sessions: 1,
    }
}

/// 6 状態を一巡だけ、`hold` 間隔で流す。
pub fn play_demo_once<R: Runtime>(app: AppHandle<R>, hold: Duration) {
    thread::spawn(move || {
        for state in ALL_STATES {
            publish(&app, demo_snapshot(state));
            thread::sleep(hold);
        }
        publish(&app, demo_snapshot(AssistantState::Idle));
    });
}

/// 環境変数 `SECRETARY_DEMO=1` のとき、6 状態を繰り返し流し続ける(開発確認用)。
pub fn start_demo_loop_if_requested<R: Runtime>(app: AppHandle<R>) {
    if std::env::var("SECRETARY_DEMO")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        eprintln!("[demo] SECRETARY_DEMO=1: cycling through states every 3s");
        thread::spawn(move || loop {
            for state in ALL_STATES {
                publish(&app, demo_snapshot(state));
                thread::sleep(Duration::from_secs(3));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_names_round_trip() {
        for state in ALL_STATES {
            assert_eq!(state_from_name(state_name(state)), Some(state));
        }
        assert_eq!(state_from_name("nope"), None);
    }

    #[test]
    fn demo_snapshot_carries_status_and_waiting_permission() {
        assert_eq!(demo_snapshot(AssistantState::Idle).message, None);
        let waiting = demo_snapshot(AssistantState::Waiting);
        assert_eq!(waiting.status, AssistantState::Waiting);
        assert_eq!(waiting.pending_permission.as_deref(), Some("Bash"));
        assert_eq!(waiting.message_kind, Some(SpeechKind::Permission));
        assert!(waiting.relayed_permission.is_some());
        assert!(demo_snapshot(AssistantState::Working)
            .relayed_permission
            .is_none());
    }
}
