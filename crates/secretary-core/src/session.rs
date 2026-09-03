//! セッション単位の状態機械。`docs/state-machine.md` の規則をそのまま実装する。

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::channel::RelayedPermission;
use crate::classify::{classify, display_tool_name, reply_text, ToolClass};
use crate::hook::{split_channel_tag, HookEvent, ToolRef};
use crate::snapshot::SpeechKind;
use crate::state::AssistantState;

/// 一時状態の保持時間などの設定。
#[derive(Debug, Clone)]
pub struct HoldConfig {
    pub success_hold: Duration,
    pub error_hold: Duration,
    /// 吹き出し文言の最大文字数(超えた分は「…」に置き換える)
    pub max_message_chars: usize,
}

impl Default for HoldConfig {
    fn default() -> Self {
        Self {
            success_hold: Duration::from_secs(3),
            error_hold: Duration::from_secs(5),
            max_message_chars: 120,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Speech {
    pub text: String,
    pub kind: SpeechKind,
}

#[derive(Debug, Clone)]
struct InFlight {
    use_id: Option<String>,
    name: String,
    class: ToolClass,
    label: String,
}

#[derive(Debug, Clone, Copy)]
struct Transient {
    state: AssistantState,
    until: Instant,
}

#[derive(Debug, Clone)]
pub struct SessionState {
    id: String,
    cwd: Option<String>,
    channel_origin: bool,
    /// channel 経由で中継され、まだ答えていない権限要求
    relayed: Option<RelayedPermission>,
    in_turn: bool,
    in_flight: Vec<InFlight>,
    turn_had_failure: bool,
    turn_has_reply: bool,
    waiting: Option<String>,
    transient: Option<Transient>,
    speech: Option<Speech>,
    task_summary: Option<String>,
    last_event_at: Instant,
    ended: bool,
}

impl SessionState {
    pub fn new(id: impl Into<String>, cwd: Option<String>, now: Instant) -> Self {
        Self {
            id: id.into(),
            cwd,
            channel_origin: false,
            relayed: None,
            in_turn: false,
            in_flight: Vec::new(),
            turn_had_failure: false,
            turn_has_reply: false,
            waiting: None,
            transient: None,
            speech: None,
            task_summary: None,
            last_event_at: now,
            ended: false,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    pub fn set_cwd_if_missing(&mut self, cwd: Option<&str>) {
        if self.cwd.is_none() {
            self.cwd = cwd.map(str::to_string);
        }
    }

    /// 追跡対象の channel(Discord か秘書)由来のプロンプトを一度でも受け取ったか。
    /// 秘書 channel から権限要求が中継されてきたセッションも含む(channel として登録された証拠なので)。
    pub fn channel_origin(&self) -> bool {
        self.channel_origin
    }

    pub fn ended(&self) -> bool {
        self.ended
    }

    pub fn last_event_at(&self) -> Instant {
        self.last_event_at
    }

    pub fn speech(&self) -> Option<&Speech> {
        self.speech.as_ref()
    }

    pub fn task_summary(&self) -> Option<&str> {
        self.task_summary.as_deref()
    }

    pub fn pending_permission(&self) -> Option<&str> {
        self.waiting.as_deref()
    }

    /// channel 経由で中継され、秘書から許可 / 拒否を返せる権限要求。
    pub fn relayed_permission(&self) -> Option<&RelayedPermission> {
        self.relayed.as_ref()
    }

    /// 中継された権限要求を登録する。hook の PermissionRequest より先に届くこともあるので、
    /// 許可待ちの状態と文言もここで揃える。
    pub fn note_relayed_permission(
        &mut self,
        permission: RelayedPermission,
        now: Instant,
        cfg: &HoldConfig,
    ) {
        self.last_event_at = now;
        self.channel_origin = true;
        self.in_turn = true;
        let name = display_tool_name(&permission.tool_name);
        if self.speech.as_ref().map(|s| s.kind) != Some(SpeechKind::Permission) {
            self.set_speech(
                format!("{name} の実行許可を待っています"),
                SpeechKind::Permission,
                cfg,
            );
        }
        if self.waiting.is_none() {
            self.waiting = Some(name);
        }
        self.relayed = Some(permission);
    }

    /// 秘書から答えたので中継分だけを消す。許可待ちの状態自体は hook の解決
    /// (PostToolUse / PermissionDenied)が届くまで保つ。
    pub fn clear_relayed_permission(&mut self) {
        self.relayed = None;
    }

    /// 秘書から答えた。答えは Claude Code に届いて許可待ちを解くので、表示も先に進める。
    pub fn resolve_relayed_permission(&mut self) {
        self.relayed = None;
        self.clear_waiting();
    }

    /// `reply` ツール経由で届いた Claude の返信。hook の PreToolUse と同じ扱いにする。
    pub fn note_reply(&mut self, text: &str, now: Instant, cfg: &HoldConfig) {
        self.last_event_at = now;
        self.set_speech(text.to_string(), SpeechKind::Reply, cfg);
        self.turn_has_reply = true;
    }

    /// 人が読めるラベル。cwd の末尾ディレクトリ名、無ければ session_id の先頭 8 文字。
    pub fn label(&self) -> String {
        self.cwd
            .as_deref()
            .and_then(|c| c.trim_end_matches('/').rsplit('/').next())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.id.chars().take(8).collect())
    }

    /// 実行中ツールの説明。作業系を優先し、無ければ最後に始まったもの。
    pub fn current_tool(&self) -> Option<&str> {
        self.in_flight
            .iter()
            .rev()
            .find(|t| matches!(t.class, ToolClass::Working | ToolClass::Reply))
            .or_else(|| self.in_flight.last())
            .map(|t| t.label.as_str())
    }

    pub fn is_stale(&self, now: Instant, ttl: Duration) -> bool {
        now.saturating_duration_since(self.last_event_at) >= ttl
    }

    /// 現在の表示状態を導出する。
    pub fn state(&self, now: Instant) -> AssistantState {
        if let Some(t) = self.transient {
            if now < t.until {
                return t.state;
            }
        }
        if self.waiting.is_some() {
            return AssistantState::Waiting;
        }
        if self
            .in_flight
            .iter()
            .any(|t| matches!(t.class, ToolClass::Working | ToolClass::Reply))
        {
            return AssistantState::Working;
        }
        if self.in_turn {
            return AssistantState::Thinking;
        }
        AssistantState::Idle
    }

    pub fn apply(&mut self, event: &HookEvent, now: Instant, cfg: &HoldConfig) {
        self.last_event_at = now;
        match event {
            HookEvent::SessionStart { .. } => {
                let cwd = self.cwd.take();
                *self = SessionState::new(self.id.clone(), cwd, now);
            }
            HookEvent::SessionEnd { .. } => {
                self.ended = true;
            }
            HookEvent::UserPromptSubmit { prompt, .. } => {
                let origin = split_channel_tag(prompt);
                self.channel_origin |= origin.from_followed_channel();
                self.task_summary = first_line(&origin.body, cfg.max_message_chars);
                self.in_turn = true;
                self.in_flight.clear();
                self.turn_had_failure = false;
                self.turn_has_reply = false;
                self.waiting = None;
                self.relayed = None;
                self.transient = None;
                self.speech = None;
            }
            HookEvent::PreToolUse { tool } => {
                self.in_turn = true;
                self.clear_waiting();
                self.transient = None;
                let class = classify(&tool.name, &tool.input);
                if class == ToolClass::Reply {
                    if let Some(text) = reply_text(&tool.input) {
                        self.set_speech(text, SpeechKind::Reply, cfg);
                        self.turn_has_reply = true;
                    }
                }
                self.in_flight.push(InFlight {
                    use_id: tool.use_id.clone(),
                    name: tool.name.clone(),
                    class,
                    label: tool_label(tool),
                });
            }
            HookEvent::PostToolUse { tool } => {
                self.remove_in_flight(tool);
                self.clear_waiting();
            }
            HookEvent::PostToolUseFailure { tool, error, .. } => {
                self.remove_in_flight(tool);
                self.clear_waiting();
                self.turn_had_failure = true;
                self.transient = Some(Transient {
                    state: AssistantState::Error,
                    until: now + cfg.error_hold,
                });
                let head = first_line(error, cfg.max_message_chars).unwrap_or_default();
                self.set_speech(
                    format!("{} が失敗しました: {head}", tool.name),
                    SpeechKind::System,
                    cfg,
                );
            }
            HookEvent::PermissionRequest { tool } => {
                self.in_turn = true;
                let name = display_tool_name(&tool.name);
                self.set_speech(
                    format!("{name} の実行許可を待っています"),
                    SpeechKind::Permission,
                    cfg,
                );
                self.waiting = Some(name);
            }
            HookEvent::PermissionDenied { tool, .. } => {
                self.remove_in_flight(tool);
                self.clear_waiting();
            }
            HookEvent::Notification {
                notification_type,
                message,
            } => {
                if notification_type.as_deref() == Some("permission_prompt")
                    && self.waiting.is_none()
                {
                    let what = self
                        .in_flight
                        .last()
                        .map(|t| t.name.clone())
                        .or_else(|| message.clone())
                        .unwrap_or_else(|| "permission".to_string());
                    self.waiting = Some(what);
                }
            }
            HookEvent::Stop {
                last_assistant_message,
                ..
            } => {
                self.in_turn = false;
                self.in_flight.clear();
                self.clear_waiting();
                self.transient = if self.turn_had_failure {
                    None
                } else {
                    Some(Transient {
                        state: AssistantState::Success,
                        until: now + cfg.success_hold,
                    })
                };
                if !self.turn_has_reply {
                    if let Some(text) = last_assistant_message
                        .as_deref()
                        .filter(|t| !t.trim().is_empty())
                    {
                        self.set_speech(text.to_string(), SpeechKind::Assistant, cfg);
                    }
                }
            }
            HookEvent::StopFailure { error, .. } => {
                self.in_turn = false;
                self.in_flight.clear();
                self.clear_waiting();
                self.turn_had_failure = true;
                self.transient = Some(Transient {
                    state: AssistantState::Error,
                    until: now + cfg.error_hold,
                });
                let label = error.clone().unwrap_or_else(|| "unknown".to_string());
                self.set_speech(
                    format!("応答に失敗しました ({label})"),
                    SpeechKind::System,
                    cfg,
                );
            }
            HookEvent::Other(_) => {}
        }
    }

    /// 許可待ちを解除する。権限要求の文言は用済みなので一緒に消す。
    fn clear_waiting(&mut self) {
        self.waiting = None;
        self.relayed = None;
        if self.speech.as_ref().map(|s| s.kind) == Some(SpeechKind::Permission) {
            self.speech = None;
        }
    }

    fn remove_in_flight(&mut self, tool: &ToolRef) {
        let pos = match &tool.use_id {
            Some(id) => self
                .in_flight
                .iter()
                .position(|t| t.use_id.as_deref() == Some(id)),
            None => None,
        }
        .or_else(|| self.in_flight.iter().rposition(|t| t.name == tool.name));
        if let Some(i) = pos {
            self.in_flight.remove(i);
        }
    }

    fn set_speech(&mut self, text: String, kind: SpeechKind, cfg: &HoldConfig) {
        self.speech = Some(Speech {
            text: truncate_chars(text.trim(), cfg.max_message_chars),
            kind,
        });
    }
}

fn tool_label(tool: &ToolRef) -> String {
    let detail = match tool.name.as_str() {
        "Bash" | "PowerShell" => tool
            .input
            .get("command")
            .and_then(Value::as_str)
            .and_then(|c| c.lines().next())
            .map(str::to_string),
        _ => tool
            .input
            .get("file_path")
            .or_else(|| tool.input.get("path"))
            .or_else(|| tool.input.get("pattern"))
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    match detail {
        Some(d) => format!("{}: {}", tool.name, truncate_chars(d.trim(), 60)),
        None => tool.name.clone(),
    }
}

fn first_line(text: &str, max: usize) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(truncate_chars(line, max))
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn tool(name: &str, id: &str, input: Value) -> ToolRef {
        ToolRef {
            name: name.into(),
            use_id: Some(id.into()),
            input,
        }
    }

    fn pre(name: &str, id: &str, input: Value) -> HookEvent {
        HookEvent::PreToolUse {
            tool: tool(name, id, input),
        }
    }

    fn post(name: &str, id: &str) -> HookEvent {
        HookEvent::PostToolUse {
            tool: tool(name, id, json!({})),
        }
    }

    fn prompt(text: &str) -> HookEvent {
        HookEvent::UserPromptSubmit {
            prompt: text.into(),
            prompt_id: None,
        }
    }

    fn stop(msg: &str) -> HookEvent {
        HookEvent::Stop {
            last_assistant_message: Some(msg.into()),
            stop_hook_active: false,
        }
    }

    struct Harness {
        s: SessionState,
        now: Instant,
        cfg: HoldConfig,
    }

    impl Harness {
        fn new() -> Self {
            let now = Instant::now();
            Self {
                s: SessionState::new("sess", Some("/repo/app".into()), now),
                now,
                cfg: HoldConfig::default(),
            }
        }
        fn apply(&mut self, e: HookEvent) -> AssistantState {
            self.now += Duration::from_millis(100);
            self.s.apply(&e, self.now, &self.cfg);
            self.s.state(self.now)
        }
        fn advance(&mut self, d: Duration) -> AssistantState {
            self.now += d;
            self.s.state(self.now)
        }
    }

    #[test]
    fn basic_turn_goes_thinking_working_success_idle() {
        let mut h = Harness::new();
        assert_eq!(h.s.state(h.now), AssistantState::Idle);
        assert_eq!(h.apply(prompt("直して")), AssistantState::Thinking);
        assert_eq!(
            h.apply(pre("Read", "a", json!({"file_path": "src/x.rs"}))),
            AssistantState::Thinking
        );
        assert_eq!(h.apply(post("Read", "a")), AssistantState::Thinking);
        assert_eq!(
            h.apply(pre("Edit", "b", json!({"file_path": "src/x.rs"}))),
            AssistantState::Working
        );
        assert_eq!(h.s.current_tool(), Some("Edit: src/x.rs"));
        assert_eq!(h.apply(post("Edit", "b")), AssistantState::Thinking);
        assert_eq!(h.apply(stop("直しました")), AssistantState::Success);
        assert_eq!(h.s.speech().unwrap().kind, SpeechKind::Assistant);
        assert_eq!(h.advance(Duration::from_secs(4)), AssistantState::Idle);
        assert_eq!(h.s.task_summary(), Some("直して"));
    }

    #[test]
    fn bash_read_only_is_thinking_but_touch_is_working() {
        let mut h = Harness::new();
        h.apply(prompt("見て"));
        assert_eq!(
            h.apply(pre("Bash", "a", json!({"command": "cat README.md"}))),
            AssistantState::Thinking
        );
        h.apply(post("Bash", "a"));
        assert_eq!(
            h.apply(pre("Bash", "b", json!({"command": "touch /tmp/x"}))),
            AssistantState::Working
        );
        assert_eq!(h.s.current_tool(), Some("Bash: touch /tmp/x"));
    }

    #[test]
    fn permission_request_is_waiting_until_tool_finishes() {
        let mut h = Harness::new();
        h.apply(prompt("作って"));
        h.apply(pre("Bash", "a", json!({"command": "touch /tmp/x"})));
        assert_eq!(
            h.apply(HookEvent::PermissionRequest {
                tool: ToolRef {
                    name: "Bash".into(),
                    use_id: None,
                    input: json!({})
                }
            }),
            AssistantState::Waiting
        );
        assert_eq!(h.s.pending_permission(), Some("Bash"));
        assert_eq!(h.s.speech().unwrap().kind, SpeechKind::Permission);
        assert_eq!(h.apply(post("Bash", "a")), AssistantState::Thinking);
        assert_eq!(h.s.pending_permission(), None);
        // 許可待ちの文言は解消と同時に消える
        assert!(h.s.speech().is_none());
    }

    #[test]
    fn resolving_permission_keeps_non_permission_speech() {
        let mut h = Harness::new();
        h.apply(prompt("作って"));
        h.apply(pre(
            "mcp__plugin_discord_discord__reply",
            "r",
            json!({"chat_id": "1", "text": "やります"}),
        ));
        h.apply(post("mcp__plugin_discord_discord__reply", "r"));
        h.apply(pre("Bash", "a", json!({"command": "touch /tmp/x"})));
        h.apply(HookEvent::PermissionRequest {
            tool: ToolRef {
                name: "Bash".into(),
                use_id: None,
                input: json!({}),
            },
        });
        assert_eq!(h.s.speech().unwrap().kind, SpeechKind::Permission);
        h.apply(post("Bash", "a"));
        // 権限文言は消えるが、返信文言は戻らない(上書き済み)。次の発言まで空
        assert!(h.s.speech().is_none());
    }

    #[test]
    fn stop_clears_leaked_in_flight_after_silent_denial() {
        let mut h = Harness::new();
        h.apply(prompt("作って"));
        assert_eq!(
            h.apply(pre("Bash", "a", json!({"command": "touch /tmp/x"}))),
            AssistantState::Working
        );
        // 拒否されると PostToolUse が来ない(Phase 0 で観測)
        assert_eq!(h.apply(stop("denied")), AssistantState::Success);
        assert_eq!(h.advance(Duration::from_secs(4)), AssistantState::Idle);
        assert_eq!(h.s.current_tool(), None);
    }

    #[test]
    fn failure_shows_error_then_working_overrides_and_stop_is_not_success() {
        let mut h = Harness::new();
        h.apply(prompt("テストして"));
        h.apply(pre("Bash", "a", json!({"command": "bun test"})));
        assert_eq!(
            h.apply(HookEvent::PostToolUseFailure {
                tool: tool("Bash", "a", json!({})),
                error: "Exit code 1\nfailed".into(),
                is_interrupt: false,
            }),
            AssistantState::Error
        );
        assert_eq!(
            h.s.speech().unwrap().text,
            "Bash が失敗しました: Exit code 1"
        );
        assert_eq!(
            h.apply(pre("Edit", "b", json!({"file_path": "a.rs"}))),
            AssistantState::Working
        );
        h.apply(post("Edit", "b"));
        assert_eq!(h.apply(stop("直せませんでした")), AssistantState::Idle);
    }

    #[test]
    fn error_transient_expires_back_to_thinking() {
        let mut h = Harness::new();
        h.apply(prompt("x"));
        h.apply(pre("Bash", "a", json!({"command": "bun test"})));
        h.apply(HookEvent::PostToolUseFailure {
            tool: tool("Bash", "a", json!({})),
            error: "e".into(),
            is_interrupt: false,
        });
        assert_eq!(h.advance(Duration::from_secs(6)), AssistantState::Thinking);
    }

    #[test]
    fn parallel_tools_stay_working_until_all_finish() {
        let mut h = Harness::new();
        h.apply(prompt("x"));
        h.apply(pre("Bash", "a", json!({"command": "cargo build"})));
        h.apply(pre("Edit", "b", json!({"file_path": "a.rs"})));
        assert_eq!(h.apply(post("Bash", "a")), AssistantState::Working);
        assert_eq!(h.apply(post("Edit", "b")), AssistantState::Thinking);
    }

    #[test]
    fn reply_text_becomes_speech_and_wins_over_last_assistant_message() {
        let mut h = Harness::new();
        let discord = "<channel source=\"plugin:discord:discord\" chat_id=\"1\">\nREADMEを要約して\n</channel>";
        h.apply(prompt(discord));
        assert!(h.s.channel_origin());
        assert_eq!(h.s.task_summary(), Some("READMEを要約して"));
        assert_eq!(
            h.apply(pre(
                "mcp__plugin_discord_discord__reply",
                "r",
                json!({"chat_id": "1", "text": "作りました"})
            )),
            AssistantState::Working
        );
        h.apply(post("mcp__plugin_discord_discord__reply", "r"));
        h.apply(stop("Discord への返信を送信しました。"));
        let speech = h.s.speech().unwrap();
        assert_eq!(speech.kind, SpeechKind::Reply);
        assert_eq!(speech.text, "作りました");
    }

    #[test]
    fn new_prompt_clears_success_and_speech() {
        let mut h = Harness::new();
        h.apply(prompt("a"));
        h.apply(stop("done"));
        assert_eq!(h.apply(prompt("b")), AssistantState::Thinking);
        assert!(h.s.speech().is_none());
    }

    #[test]
    fn notification_permission_prompt_sets_waiting_with_last_tool_name() {
        let mut h = Harness::new();
        h.apply(prompt("a"));
        h.apply(pre("Bash", "a", json!({"command": "rm -rf build"})));
        assert_eq!(
            h.apply(HookEvent::Notification {
                notification_type: Some("permission_prompt".into()),
                message: Some("Claude needs your permission".into())
            }),
            AssistantState::Waiting
        );
        assert_eq!(h.s.pending_permission(), Some("Bash"));
    }

    #[test]
    fn stop_failure_is_error_then_idle() {
        let mut h = Harness::new();
        h.apply(prompt("a"));
        assert_eq!(
            h.apply(HookEvent::StopFailure {
                error: Some("rate_limit".into()),
                last_assistant_message: None
            }),
            AssistantState::Error
        );
        assert_eq!(h.advance(Duration::from_secs(6)), AssistantState::Idle);
    }

    #[test]
    fn session_end_marks_ended_and_stale_detection_works() {
        let mut h = Harness::new();
        h.apply(HookEvent::SessionEnd {
            reason: Some("other".into()),
        });
        assert!(h.s.ended());
        assert!(!h.s.is_stale(h.now, Duration::from_secs(60)));
        assert!(h
            .s
            .is_stale(h.now + Duration::from_secs(61), Duration::from_secs(60)));
    }

    #[test]
    fn label_uses_last_path_component() {
        let h = Harness::new();
        assert_eq!(h.s.label(), "app");
    }

    #[test]
    fn long_messages_are_truncated_by_chars() {
        let mut h = Harness::new();
        h.cfg.max_message_chars = 5;
        h.apply(prompt("a"));
        h.apply(stop("あいうえおかきくけこ"));
        assert_eq!(h.s.speech().unwrap().text, "あいうえ…");
    }
}
