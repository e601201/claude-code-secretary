//! Secretary Core
//!
//! Claude Code の hook イベントを受け取り、秘書キャラクターが表示すべき状態を導出する。
//! Tauri や HTTP には依存しない。時刻は呼び出し側から [`std::time::Instant`] で渡すので、
//! テストでは時間を自由に進められる。
//!
//! 流れ: JSON → [`HookEnvelope`] → [`HookEvent`] → [`SessionState::apply`] → [`Tracker::snapshot`]

pub mod channel;
pub mod classify;
pub mod hook;
pub mod session;
pub mod snapshot;
pub mod state;
pub mod tracker;

pub use channel::{ChannelCommand, ChannelEvent, RelayedPermission};
pub use classify::{display_tool_name, ToolClass};
pub use hook::{
    split_channel_tag, HookEnvelope, HookEvent, PromptOrigin, ToolRef, DISCORD_CHANNEL_SOURCE,
    SECRETARY_CHANNEL_SOURCE,
};
pub use session::{HoldConfig, SessionState, Speech};
pub use snapshot::{SecretarySnapshot, SpeechKind};
pub use state::AssistantState;
pub use tracker::{FollowPolicy, Tracker, TrackerConfig};
