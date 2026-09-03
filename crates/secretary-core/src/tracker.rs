//! 複数セッションを束ね、表示対象を選び、スナップショットを作る。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::hook::{HookEnvelope, HookEvent};
use crate::session::{HoldConfig, SessionState};
use crate::snapshot::SecretarySnapshot;

/// どのセッションを表示対象にするか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowPolicy {
    /// Discord 由来のプロンプトを受け取ったことのあるセッションだけ(既定)
    Discord,
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
            follow: FollowPolicy::Discord,
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
            FollowPolicy::Discord => s.discord_origin(),
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
