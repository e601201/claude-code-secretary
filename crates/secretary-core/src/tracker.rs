//! 複数セッションを束ね、表示対象を選び、スナップショットを作る。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::channel::RelayedPermission;
use crate::hook::{HookEnvelope, HookEvent};
use crate::session::{HoldConfig, SessionState};
use crate::snapshot::SecretarySnapshot;

/// どのセッションを表示対象にするか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowPolicy {
    /// channel(Discord か秘書自身)由来のプロンプトを受け取ったことのあるセッションだけ(既定)
    Channel,
    /// すべてのセッション
    All,
    /// 指定した session_id だけ
    Session(String),
}

#[derive(Debug, Clone)]
pub struct TrackerConfig {
    pub hold: HoldConfig,
    /// この時間イベントが無いセッションは失効させる
    pub stale_after: Duration,
    pub follow: FollowPolicy,
    /// 空でなければ、cwd がこのいずれかで始まるセッションだけを対象にする
    pub cwd_prefixes: Vec<String>,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            hold: HoldConfig::default(),
            stale_after: Duration::from_secs(30 * 60),
            follow: FollowPolicy::Channel,
            cwd_prefixes: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct Tracker {
    cfg: TrackerConfig,
    sessions: HashMap<String, SessionState>,
}

impl Tracker {
    pub fn new(cfg: TrackerConfig) -> Self {
        Self {
            cfg,
            sessions: HashMap::new(),
        }
    }

    pub fn config(&self) -> &TrackerConfig {
        &self.cfg
    }

    pub fn set_follow(&mut self, policy: FollowPolicy) {
        self.cfg.follow = policy;
    }

    /// 設定を丸ごと差し替える(設定画面の保存)。セッションはそのまま。
    pub fn update_config(&mut self, cfg: TrackerConfig) {
        self.cfg = cfg;
    }

    /// JSON 文字列をそのまま適用する。hook スクリプトや HTTP サーバーから呼ぶ入口。
    pub fn apply_json(&mut self, json: &str, now: Instant) -> serde_json::Result<HookEvent> {
        let env = HookEnvelope::parse(json)?;
        Ok(self.apply(&env, now))
    }

    pub fn apply(&mut self, env: &HookEnvelope, now: Instant) -> HookEvent {
        let event = env.event();
        let session = self
            .sessions
            .entry(env.session_id.clone())
            .or_insert_with(|| SessionState::new(env.session_id.clone(), env.cwd.clone(), now));
        session.set_cwd_if_missing(env.cwd.as_deref());
        session.apply(&event, now, &self.cfg.hold);
        if session.ended() {
            self.sessions.remove(&env.session_id);
        }
        event
    }

    /// channel から中継された権限要求を登録する。未知のセッションなら作る。
    pub fn note_relayed_permission(
        &mut self,
        session_id: &str,
        cwd: Option<&str>,
        permission: RelayedPermission,
        now: Instant,
    ) {
        let hold = self.cfg.hold.clone();
        self.session_entry(session_id, cwd, now)
            .note_relayed_permission(permission, now, &hold);
    }

    /// channel が切れたのでボタンだけを消す。許可待ちの状態は hook の解決まで保つ。
    pub fn clear_relayed_permission(&mut self, session_id: &str) {
        if let Some(s) = self.sessions.get_mut(session_id) {
            s.clear_relayed_permission();
        }
    }

    /// 秘書が権限要求に答えた。ボタンを消し、許可待ちも解く。
    pub fn resolve_relayed_permission(&mut self, session_id: &str) {
        if let Some(s) = self.sessions.get_mut(session_id) {
            s.resolve_relayed_permission();
        }
    }

    /// `reply` ツール経由の返信を反映する。
    pub fn note_reply(&mut self, session_id: &str, cwd: Option<&str>, text: &str, now: Instant) {
        let hold = self.cfg.hold.clone();
        self.session_entry(session_id, cwd, now)
            .note_reply(text, now, &hold);
    }

    fn session_entry(&mut self, id: &str, cwd: Option<&str>, now: Instant) -> &mut SessionState {
        let session = self
            .sessions
            .entry(id.to_string())
            .or_insert_with(|| SessionState::new(id, cwd.map(str::to_string), now));
        session.set_cwd_if_missing(cwd);
        session
    }

    /// 失効したセッションを削除し、削除数を返す。
    pub fn expire_stale(&mut self, now: Instant) -> usize {
        let before = self.sessions.len();
        let ttl = self.cfg.stale_after;
        self.sessions.retain(|_, s| !s.is_stale(now, ttl));
        before - self.sessions.len()
    }

    pub fn sessions(&self) -> impl Iterator<Item = &SessionState> {
        self.sessions.values()
    }

    pub fn session(&self, id: &str) -> Option<&SessionState> {
        self.sessions.get(id)
    }

    fn is_followed(&self, s: &SessionState) -> bool {
        let by_policy = match &self.cfg.follow {
            FollowPolicy::Channel => s.channel_origin(),
            FollowPolicy::All => true,
            FollowPolicy::Session(id) => s.id() == id,
        };
        if !by_policy {
            return false;
        }
        if self.cfg.cwd_prefixes.is_empty() {
            return true;
        }
        s.cwd()
            .map(|c| {
                self.cfg
                    .cwd_prefixes
                    .iter()
                    .any(|p| c.starts_with(p.as_str()))
            })
            .unwrap_or(false)
    }

    /// 指定したセッションが追跡対象か。
    ///
    /// 全文ビューは「吹き出しに映り得たもの」だけを控える。`Core` は全セッションの
    /// hook を見てしまうので、控える前にここで絞る。
    pub fn is_session_followed(&self, id: &str) -> bool {
        self.sessions.get(id).is_some_and(|s| self.is_followed(s))
    }

    /// 表示対象のセッション一覧。
    pub fn followed(&self) -> Vec<&SessionState> {
        self.sessions
            .values()
            .filter(|s| self.is_followed(s))
            .collect()
    }

    /// 表示対象の中で最も優先度の高いセッションからスナップショットを作る。
    pub fn snapshot(&self, now: Instant) -> SecretarySnapshot {
        let followed = self.followed();
        let Some(best) = followed
            .iter()
            .max_by_key(|s| (s.state(now).priority(), s.last_event_at()))
        else {
            return SecretarySnapshot::idle();
        };
        SecretarySnapshot {
            status: best.state(now),
            message: best.speech().map(|sp| sp.text.clone()),
            message_kind: best.speech().map(|sp| sp.kind),
            current_tool: best.current_tool().map(str::to_string),
            task_summary: best.task_summary().map(str::to_string),
            pending_permission: best.pending_permission().map(str::to_string),
            relayed_permission: best.relayed_permission().cloned(),
            session_id: Some(best.id().to_string()),
            session_label: Some(best.label()),
            tracked_sessions: followed.len() as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AssistantState;
    use serde_json::json;

    fn env(session: &str, cwd: &str, event: &str, extra: serde_json::Value) -> HookEnvelope {
        let mut v = json!({ "session_id": session, "cwd": cwd, "hook_event_name": event });
        if let (Some(base), Some(add)) = (v.as_object_mut(), extra.as_object()) {
            for (k, val) in add {
                base.insert(k.clone(), val.clone());
            }
        }
        serde_json::from_value(v).unwrap()
    }

    const DISCORD: &str =
        "<channel source=\"plugin:discord:discord\" chat_id=\"1\">\nやって\n</channel>";

    #[test]
    fn discord_policy_ignores_terminal_sessions() {
        let mut t = Tracker::new(TrackerConfig::default());
        let now = Instant::now();
        t.apply(
            &env(
                "dev",
                "/repo",
                "UserPromptSubmit",
                json!({"prompt": "手元の作業"}),
            ),
            now,
        );
        t.apply(
            &env(
                "dev",
                "/repo",
                "PreToolUse",
                json!({"tool_name": "Edit", "tool_use_id": "a", "tool_input": {}}),
            ),
            now,
        );
        let snap = t.snapshot(now);
        assert_eq!(snap.status, AssistantState::Idle);
        assert_eq!(snap.tracked_sessions, 0);

        t.apply(
            &env(
                "disc",
                "/repo",
                "UserPromptSubmit",
                json!({"prompt": DISCORD}),
            ),
            now,
        );
        let snap = t.snapshot(now);
        assert_eq!(snap.status, AssistantState::Thinking);
        assert_eq!(snap.session_id.as_deref(), Some("disc"));
        assert_eq!(snap.session_label.as_deref(), Some("repo"));
        assert_eq!(snap.task_summary.as_deref(), Some("やって"));
        assert_eq!(snap.tracked_sessions, 1);
    }

    const SECRETARY: &str = "<channel source=\"secretary\" chat_id=\"desk\">\n直して\n</channel>";

    #[test]
    fn secretary_prompt_is_followed_like_discord() {
        let mut t = Tracker::new(TrackerConfig::default());
        let now = Instant::now();
        t.apply(
            &env(
                "desk",
                "/repo",
                "UserPromptSubmit",
                json!({"prompt": SECRETARY}),
            ),
            now,
        );
        let snap = t.snapshot(now);
        assert_eq!(snap.session_id.as_deref(), Some("desk"));
        assert_eq!(snap.task_summary.as_deref(), Some("直して"));
    }

    #[test]
    fn relayed_permission_creates_waiting_session_and_clears_on_answer_or_hook() {
        let mut t = Tracker::new(TrackerConfig::default());
        let now = Instant::now();
        let perm = RelayedPermission {
            request_id: "abcde".into(),
            tool_name: "Bash".into(),
            description: "Run tests".into(),
            input_preview: "{\"command\":\"bun test\"}".into(),
        };
        t.note_relayed_permission("s", Some("/repo"), perm.clone(), now);
        let snap = t.snapshot(now);
        assert_eq!(snap.status, AssistantState::Waiting);
        assert_eq!(snap.pending_permission.as_deref(), Some("Bash"));
        assert_eq!(snap.relayed_permission.as_ref(), Some(&perm));
        assert_eq!(snap.session_label.as_deref(), Some("repo"));

        t.clear_relayed_permission("s");
        let snap = t.snapshot(now);
        assert_eq!(
            snap.status,
            AssistantState::Waiting,
            "接続が切れただけなら hook が解決するまで許可待ちのまま"
        );
        assert_eq!(snap.relayed_permission, None);

        t.note_relayed_permission("s", None, perm.clone(), now);
        t.resolve_relayed_permission("s");
        let snap = t.snapshot(now);
        assert_ne!(
            snap.status,
            AssistantState::Waiting,
            "秘書が答えたら許可待ちも解ける"
        );
        assert_eq!(snap.relayed_permission, None);
        assert_eq!(snap.message, None, "許可待ちの文言も消える");

        t.note_relayed_permission("s", None, perm, now);
        t.apply(
            &env(
                "s",
                "/repo",
                "PostToolUse",
                json!({"tool_name": "Bash", "tool_use_id": "1", "tool_input": {}}),
            ),
            now,
        );
        let snap = t.snapshot(now);
        assert_ne!(snap.status, AssistantState::Waiting);
        assert_eq!(snap.relayed_permission, None);
    }

    #[test]
    fn note_reply_sets_reply_speech_for_followed_session() {
        let mut t = Tracker::new(TrackerConfig::default());
        let now = Instant::now();
        t.apply(
            &env(
                "desk",
                "/repo",
                "UserPromptSubmit",
                json!({"prompt": SECRETARY}),
            ),
            now,
        );
        t.note_reply("desk", None, "やりました", now);
        let snap = t.snapshot(now);
        assert_eq!(snap.message.as_deref(), Some("やりました"));
        assert_eq!(snap.message_kind, Some(crate::SpeechKind::Reply));
    }

    #[test]
    fn is_session_followed_respects_the_policy() {
        let mut t = Tracker::new(TrackerConfig::default()); // 既定は Channel
        let now = Instant::now();
        // ターミナルで直接動かしただけのセッションは追跡対象にならない
        t.apply(
            &env("plain", "/a", "UserPromptSubmit", json!({"prompt": "x"})),
            now,
        );
        assert!(!t.is_session_followed("plain"));
        // channel 由来のプロンプトを受けたセッションは追跡対象
        t.apply(
            &env("ch", "/b", "UserPromptSubmit", json!({"prompt": DISCORD})),
            now,
        );
        assert!(t.is_session_followed("ch"));
        // 知らないセッションは対象外
        assert!(!t.is_session_followed("nope"));
    }

    #[test]
    fn all_policy_picks_highest_priority_session() {
        let mut t = Tracker::new(TrackerConfig {
            follow: FollowPolicy::All,
            ..Default::default()
        });
        let now = Instant::now();
        t.apply(
            &env("a", "/a", "UserPromptSubmit", json!({"prompt": "x"})),
            now,
        );
        t.apply(
            &env(
                "a",
                "/a",
                "PreToolUse",
                json!({"tool_name": "Edit", "tool_use_id": "1", "tool_input": {}}),
            ),
            now,
        );
        t.apply(
            &env("b", "/b", "UserPromptSubmit", json!({"prompt": "y"})),
            now,
        );
        t.apply(
            &env(
                "b",
                "/b",
                "PreToolUse",
                json!({"tool_name": "Bash", "tool_use_id": "2", "tool_input": {"command": "rm x"}}),
            ),
            now,
        );
        t.apply(
            &env(
                "b",
                "/b",
                "PermissionRequest",
                json!({"tool_name": "Bash", "tool_input": {}}),
            ),
            now,
        );
        let snap = t.snapshot(now);
        assert_eq!(snap.status, AssistantState::Waiting);
        assert_eq!(snap.session_id.as_deref(), Some("b"));
        assert_eq!(snap.pending_permission.as_deref(), Some("Bash"));
        assert_eq!(snap.tracked_sessions, 2);
    }

    #[test]
    fn cwd_prefix_filter_applies_on_top_of_policy() {
        let mut t = Tracker::new(TrackerConfig {
            follow: FollowPolicy::All,
            cwd_prefixes: vec!["/work/life".into()],
            ..Default::default()
        });
        let now = Instant::now();
        t.apply(
            &env(
                "a",
                "/work/other",
                "UserPromptSubmit",
                json!({"prompt": "x"}),
            ),
            now,
        );
        t.apply(
            &env(
                "b",
                "/work/life",
                "UserPromptSubmit",
                json!({"prompt": "y"}),
            ),
            now,
        );
        let snap = t.snapshot(now);
        assert_eq!(snap.session_id.as_deref(), Some("b"));
        assert_eq!(snap.tracked_sessions, 1);
    }

    #[test]
    fn session_end_removes_and_stale_expires() {
        let mut t = Tracker::new(TrackerConfig {
            follow: FollowPolicy::All,
            ..Default::default()
        });
        let now = Instant::now();
        t.apply(
            &env("a", "/a", "SessionStart", json!({"source": "startup"})),
            now,
        );
        t.apply(
            &env("b", "/b", "SessionStart", json!({"source": "startup"})),
            now,
        );
        assert_eq!(t.sessions().count(), 2);
        t.apply(
            &env("a", "/a", "SessionEnd", json!({"reason": "other"})),
            now,
        );
        assert_eq!(t.sessions().count(), 1);
        assert_eq!(t.expire_stale(now + Duration::from_secs(31 * 60)), 1);
        assert_eq!(t.snapshot(now).tracked_sessions, 0);
    }

    #[test]
    fn apply_json_parses_and_applies() {
        let mut t = Tracker::new(TrackerConfig {
            follow: FollowPolicy::All,
            ..Default::default()
        });
        let now = Instant::now();
        let ev = t.apply_json(r#"{"session_id":"s","hook_event_name":"UserPromptSubmit","cwd":"/x","prompt":"hi"}"#, now).unwrap();
        assert!(matches!(ev, HookEvent::UserPromptSubmit { .. }));
        assert_eq!(t.snapshot(now).status, AssistantState::Thinking);
        assert!(t.apply_json("not json", now).is_err());
    }
}
