//! UI へ渡すスナップショット。フロントエンドはこれを描くだけで、判断はしない。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::channel::RelayedPermission;
use crate::state::AssistantState;

/// 吹き出しの文言の出所。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SpeechKind {
    /// Discord への返信本文
    Reply,
    /// ターン終了時の最終応答
    Assistant,
    /// 権限要求
    Permission,
    /// エラーなどシステム起因の文言
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SecretarySnapshot {
    pub status: AssistantState,
    /// 吹き出しに出す文言。無ければ状態テンプレート(Phase 9)に任せる
    pub message: Option<String>,
    pub message_kind: Option<SpeechKind>,
    /// ステータスパネル用。実行中ツールの短い説明
    pub current_tool: Option<String>,
    /// 直近プロンプトの要約(先頭行)
    pub task_summary: Option<String>,
    /// 許可を待っているツール名
    pub pending_permission: Option<String>,
    /// channel 経由で中継され、秘書の吹き出しから許可 / 拒否を返せる権限要求
    pub relayed_permission: Option<RelayedPermission>,
    pub session_id: Option<String>,
    /// cwd の末尾ディレクトリ名など、人が読めるラベル
    pub session_label: Option<String>,
    /// 追跡対象として認識しているセッション数
    pub tracked_sessions: u32,
}

impl SecretarySnapshot {
    pub fn idle() -> Self {
        Self {
            status: AssistantState::Idle,
            message: None,
            message_kind: None,
            current_tool: None,
            task_summary: None,
            pending_permission: None,
            relayed_permission: None,
            session_id: None,
            session_label: None,
            tracked_sessions: 0,
        }
    }
}
