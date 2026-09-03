//! 秘書 → Claude Code の逆方向の経路(Phase 10)。
//!
//! Claude Code は `--dangerously-load-development-channels server:secretary` で起動すると
//! MCP サーバー `secretary-channel` をセッションごとに子プロセスとして起動する。
//! その子プロセスとアプリ本体は Unix ドメインソケットで 1 行 1 JSON のフレームを交換する。
//!
//! ```text
//!   秘書アプリ  ──ChannelCommand──▶  secretary-channel  ──MCP notification──▶  Claude Code
//!   秘書アプリ  ◀──ChannelEvent────  secretary-channel  ◀──MCP request/notif──  Claude Code
//! ```
//!
//! 型だけをここに置き、両側(Tauri アプリと CLI)が同じ定義を使う。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Tauri の identifier。設定ディレクトリ(`~/Library/Application Support/<identifier>`)の名前になる。
pub const APP_IDENTIFIER: &str = "com.nagatadaichi.tauriapp";
/// 設定ディレクトリ直下のソケットファイル名。
pub const CHANNEL_SOCKET_FILENAME: &str = "channel.sock";
/// 設定ディレクトリ直下のトークンファイル名(hook と共用)。
pub const TOKEN_FILENAME: &str = "token";
/// ソケットの場所を上書きする環境変数(テスト用)。
pub const CHANNEL_SOCKET_ENV: &str = "SECRETARY_CHANNEL_SOCKET";
/// Claude Code が MCP サーバーの子プロセスに渡すセッション ID(実機で観測)。
pub const SESSION_ID_ENV: &str = "CLAUDE_CODE_SESSION_ID";
/// 同じく、セッションの作業ディレクトリ。
pub const PROJECT_DIR_ENV: &str = "CLAUDE_PROJECT_DIR";
/// MCP サーバー名。`<channel source="secretary">` と `mcp__secretary__reply` の元になる。
pub const CHANNEL_SERVER_NAME: &str = "secretary";

/// Claude Code が channel 経由で中継してきた権限要求。吹き出しに許可 / 拒否ボタンを出す。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RelayedPermission {
    /// Claude Code が発番した 5 文字の ID。返答にそのまま返す
    pub request_id: String,
    pub tool_name: String,
    /// 何をするかの説明(コマンドそのものではない)
    pub description: String,
    /// 引数のプレビュー(JSON 風の文字列)
    pub input_preview: String,
}

/// secretary-channel → アプリ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChannelEvent {
    /// 接続直後に一度だけ送る自己紹介
    Hello {
        session_id: String,
        #[serde(default)]
        cwd: Option<String>,
        pid: u32,
        /// hook と同じトークン。アプリはこれが一致しない接続を切る
        #[serde(default)]
        token: Option<String>,
    },
    /// Claude が `reply` ツールで秘書へ返した本文
    Reply { text: String },
    /// Claude Code からの `notifications/claude/channel/permission_request`
    PermissionRequest(RelayedPermission),
}

/// アプリ → secretary-channel。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChannelCommand {
    /// 秘書からの指示をセッションへ流し込む(`notifications/claude/channel`)
    SendPrompt { text: String },
    /// 中継された権限要求に答える(`notifications/claude/channel/permission`)
    RespondPermission { request_id: String, allow: bool },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_as_tagged_json() {
        let hello = ChannelEvent::Hello {
            session_id: "s".into(),
            cwd: Some("/x".into()),
            pid: 1,
            token: None,
        };
        let json = serde_json::to_string(&hello).unwrap();
        assert!(json.starts_with(r#"{"type":"hello""#), "{json}");
        assert_eq!(serde_json::from_str::<ChannelEvent>(&json).unwrap(), hello);

        let cmd = ChannelCommand::RespondPermission {
            request_id: "abcde".into(),
            allow: true,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(
            json,
            r#"{"type":"respond_permission","request_id":"abcde","allow":true}"#
        );
    }

    #[test]
    fn permission_request_flattens_fields() {
        let json = r#"{"type":"permission_request","request_id":"abcde","tool_name":"Bash","description":"d","input_preview":"p"}"#;
        match serde_json::from_str::<ChannelEvent>(json).unwrap() {
            ChannelEvent::PermissionRequest(p) => assert_eq!(p.tool_name, "Bash"),
            other => panic!("{other:?}"),
        }
    }
}
