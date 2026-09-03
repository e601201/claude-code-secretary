//! Claude Code の hook が送ってくる JSON の受け皿。
//!
//! フィールドは公式ドキュメントと Phase 0 の観測(`docs/spike-results.md`)に基づく。
//! 知らないフィールドは `extra` に残し、欠けているフィールドは `None` にする。

use serde::Deserialize;
use serde_json::{Map, Value};

/// hook から届く生の JSON。すべてのイベントに共通の形。
#[derive(Debug, Clone, Deserialize)]
pub struct HookEnvelope {
    pub session_id: String,
    pub hook_event_name: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub prompt_id: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub tool_input: Option<Value>,
    #[serde(default)]
    pub tool_response: Option<Value>,
    #[serde(default)]
    pub last_assistant_message: Option<String>,
    #[serde(default)]
    pub notification_type: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub is_interrupt: Option<bool>,
    #[serde(default)]
    pub stop_hook_active: Option<bool>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// ツール呼び出しの参照。Pre / Post / 権限系イベントで共通。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolRef {
    pub name: String,
    /// Pre / Post / Failure / Denied には入る。PermissionRequest には入らない。
    pub use_id: Option<String>,
    pub input: Value,
}

/// 型付きのイベント。状態機械はこれだけを見る。
#[derive(Debug, Clone, PartialEq)]
pub enum HookEvent {
    SessionStart {
        source: Option<String>,
    },
    SessionEnd {
        reason: Option<String>,
    },
    UserPromptSubmit {
        prompt: String,
        prompt_id: Option<String>,
    },
    PreToolUse {
        tool: ToolRef,
    },
    PostToolUse {
        tool: ToolRef,
    },
    PostToolUseFailure {
        tool: ToolRef,
        error: String,
        is_interrupt: bool,
    },
    PermissionRequest {
        tool: ToolRef,
    },
    PermissionDenied {
        tool: ToolRef,
        reason: Option<String>,
    },
    Notification {
        notification_type: Option<String>,
        message: Option<String>,
    },
    Stop {
        last_assistant_message: Option<String>,
        stop_hook_active: bool,
    },
    StopFailure {
        error: Option<String>,
        last_assistant_message: Option<String>,
    },
    /// 購読していない、または未知のイベント。無視するがセッションの生存確認には使う。
    Other(String),
}

impl HookEnvelope {
    pub fn parse(json: &str) -> serde_json::Result<Self> {
        serde_json::from_str(json)
    }

    /// サブエージェント内から発火した hook か。
    pub fn is_subagent(&self) -> bool {
        self.agent_id.is_some()
    }

    fn tool(&self) -> ToolRef {
        ToolRef {
            name: self.tool_name.clone().unwrap_or_default(),
            use_id: self.tool_use_id.clone(),
            input: self.tool_input.clone().unwrap_or(Value::Null),
        }
    }

    pub fn event(&self) -> HookEvent {
        match self.hook_event_name.as_str() {
            "SessionStart" => HookEvent::SessionStart {
                source: self.source.clone(),
            },
            "SessionEnd" => HookEvent::SessionEnd {
                reason: self.reason.clone(),
            },
            "UserPromptSubmit" => HookEvent::UserPromptSubmit {
                prompt: self.prompt.clone().unwrap_or_default(),
                prompt_id: self.prompt_id.clone(),
            },
            "PreToolUse" => HookEvent::PreToolUse { tool: self.tool() },
            "PostToolUse" => HookEvent::PostToolUse { tool: self.tool() },
            "PostToolUseFailure" => HookEvent::PostToolUseFailure {
                tool: self.tool(),
                error: self.error.clone().unwrap_or_default(),
                is_interrupt: self.is_interrupt.unwrap_or(false),
            },
            "PermissionRequest" => HookEvent::PermissionRequest { tool: self.tool() },
            "PermissionDenied" => HookEvent::PermissionDenied {
                tool: self.tool(),
                reason: self.reason.clone(),
            },
            "Notification" => HookEvent::Notification {
                notification_type: self.notification_type.clone(),
                message: self.message.clone(),
            },
            "Stop" => HookEvent::Stop {
                last_assistant_message: self.last_assistant_message.clone(),
                stop_hook_active: self.stop_hook_active.unwrap_or(false),
            },
            "StopFailure" => HookEvent::StopFailure {
                error: self.error.clone(),
                last_assistant_message: self.last_assistant_message.clone(),
            },
            other => HookEvent::Other(other.to_string()),
        }
    }
}

/// Discord プラグイン経由のプロンプトの `source` 属性(Phase 0 で観測)。
pub const DISCORD_CHANNEL_SOURCE: &str = "plugin:discord:discord";
/// 秘書アプリ自身の channel(Phase 10)。ユーザースコープの MCP サーバー名がそのまま入る。
pub const SECRETARY_CHANNEL_SOURCE: &str = "secretary";

/// プロンプトの出所と、チャンネルタグを取り除いた本文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptOrigin {
    /// `<channel source="…">` の source。通常のターミナル入力なら None
    pub source: Option<String>,
    pub body: String,
}

impl PromptOrigin {
    pub fn from_discord(&self) -> bool {
        self.source.as_deref() == Some(DISCORD_CHANNEL_SOURCE)
    }

    pub fn from_secretary(&self) -> bool {
        self.source.as_deref() == Some(SECRETARY_CHANNEL_SOURCE)
    }

    /// 秘書が既定で追跡対象とみなす channel(Discord か秘書自身)由来か。
    pub fn from_followed_channel(&self) -> bool {
        self.from_discord() || self.from_secretary()
    }
}

/// `<channel …>本文</channel>` 形式ならタグを剥がし、source 属性を取り出す。
pub fn split_channel_tag(prompt: &str) -> PromptOrigin {
    let trimmed = prompt.trim_start();
    if !trimmed.starts_with("<channel ") {
        return PromptOrigin {
            source: None,
            body: prompt.trim().to_string(),
        };
    }
    let tag_end = trimmed.find('>').unwrap_or(trimmed.len());
    let tag = &trimmed[..tag_end];
    let source = tag.find("source=\"").and_then(|i| {
        let rest = &tag[i + "source=\"".len()..];
        rest.find('"').map(|j| rest[..j].to_string())
    });
    let after_tag = trimmed.get(tag_end + 1..).unwrap_or("");
    let body = match after_tag.rfind("</channel>") {
        Some(end) => &after_tag[..end],
        None => after_tag,
    };
    PromptOrigin {
        source,
        body: body.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISCORD_PROMPT: &str = "<channel source=\"plugin:discord:discord\" chat_id=\"1\" message_id=\"2\" user=\"u\" user_id=\"3\" ts=\"2026-09-03T02:10:10.204Z\">\nREADMEを要約して\n</channel>";

    #[test]
    fn detects_discord_origin_and_strips_tag() {
        let origin = split_channel_tag(DISCORD_PROMPT);
        assert!(origin.from_discord());
        assert_eq!(origin.body, "READMEを要約して");
    }

    #[test]
    fn plain_prompt_is_not_discord() {
        let origin = split_channel_tag("  ふつうの指示  ");
        assert!(!origin.from_discord());
        assert_eq!(origin.body, "ふつうの指示");
    }

    #[test]
    fn other_channel_is_not_discord_but_still_stripped() {
        let origin =
            split_channel_tag("<channel source=\"plugin:slack:slack\" x=\"1\">hi</channel>");
        assert!(!origin.from_discord());
        assert_eq!(origin.body, "hi");
    }

    #[test]
    fn secretary_channel_is_recognized() {
        let origin = split_channel_tag(
            "<channel source=\"secretary\" chat_id=\"desk\">\n直して\n</channel>",
        );
        assert_eq!(origin.source.as_deref(), Some("secretary"));
        assert!(origin.from_secretary());
        assert!(origin.from_followed_channel());
        assert!(!origin.from_discord());
        assert_eq!(origin.body, "直して");
    }

    #[test]
    fn unterminated_tag_yields_empty_body() {
        let origin = split_channel_tag("<channel source=\"x\"");
        assert_eq!(origin.source.as_deref(), Some("x"));
        assert_eq!(origin.body, "");
    }

    #[test]
    fn parses_pre_tool_use_with_unknown_fields() {
        let json = r#"{"session_id":"s","hook_event_name":"PreToolUse","cwd":"/p","tool_name":"Bash",
            "tool_use_id":"toolu_1","tool_input":{"command":"ls"},"prompt_id":"p1","future_field":42}"#;
        let env = HookEnvelope::parse(json).unwrap();
        assert_eq!(env.extra.get("future_field"), Some(&Value::from(42)));
        match env.event() {
            HookEvent::PreToolUse { tool } => {
                assert_eq!(tool.name, "Bash");
                assert_eq!(tool.use_id.as_deref(), Some("toolu_1"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn unknown_event_becomes_other() {
        let env =
            HookEnvelope::parse(r#"{"session_id":"s","hook_event_name":"PreCompact"}"#).unwrap();
        assert_eq!(env.event(), HookEvent::Other("PreCompact".into()));
    }
}
