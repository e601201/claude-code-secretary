use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 秘書の表示状態。`docs/state-machine.md` の 6 状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AssistantState {
    /// 何もしていない
    Idle,
    /// 応答を考えている、または読み取り系ツールを実行中
    Thinking,
    /// 編集やコマンド実行など変更を伴う作業中
    Working,
    /// 権限要求などユーザーの応答待ち
    Waiting,
    /// ターンが正常に終わった直後(一時状態)
    Success,
    /// ツール失敗やターン失敗(一時状態)
    Error,
}

impl AssistantState {
    /// 複数セッションを束ねて表示するときの優先度。大きいほど優先。
    pub fn priority(self) -> u8 {
        match self {
            AssistantState::Waiting => 5,
            AssistantState::Error => 4,
            AssistantState::Working => 3,
            AssistantState::Thinking => 2,
            AssistantState::Success => 1,
            AssistantState::Idle => 0,
        }
    }

    /// タイマーで自動的に解除される一時状態か。
    pub fn is_transient(self) -> bool {
        matches!(self, AssistantState::Success | AssistantState::Error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_orders_waiting_first_and_idle_last() {
        let mut all = [
            AssistantState::Idle,
            AssistantState::Success,
            AssistantState::Thinking,
            AssistantState::Working,
            AssistantState::Error,
            AssistantState::Waiting,
        ];
        all.sort_by_key(|s| std::cmp::Reverse(s.priority()));
        assert_eq!(all[0], AssistantState::Waiting);
        assert_eq!(all[5], AssistantState::Idle);
    }

    #[test]
    fn serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&AssistantState::Working).unwrap(),
            "\"working\""
        );
    }
}
