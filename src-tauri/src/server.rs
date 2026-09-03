//! hook を受け取る HTTP サーバーと、Tracker を包む Core。
//!
//! - `POST /hook`   hook の JSON をそのまま受け取る。`Authorization: Bearer <token>` 必須
//! - `GET  /health` 稼働確認
//!
//! Core は Tauri に依存しないので、そのまま単体テストできる。

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    body::Bytes,
    extract::State,
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    routing::{get, post},
    Router,
};
use secretary_core::{FollowPolicy, HookEnvelope, HookEvent, SecretarySnapshot, Tracker};
use tauri::{AppHandle, Runtime};

use crate::{bridge, persona::Persona};

/// Tracker と人格、最後に配信したスナップショット。
pub struct Core {
    tracker: Mutex<Tracker>,
    persona: Mutex<Persona>,
    token: String,
    last_published: Mutex<Option<SecretarySnapshot>>,
}

impl Core {
    pub fn new(tracker: Tracker, persona: Persona, token: String) -> Self {
        Self {
            tracker: Mutex::new(tracker),
            persona: Mutex::new(persona),
            token,
            last_published: Mutex::new(None),
        }
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// hook JSON を適用する。
    pub fn ingest(&self, body: &str) -> Result<HookEvent, String> {
        let envelope = HookEnvelope::parse(body).map_err(|e| e.to_string())?;
        eprintln!("[hook] {}", summarize(&envelope));
        let now = Instant::now();
        let event = self.tracker.lock().unwrap().apply(&envelope, now);
        self.persona
            .lock()
            .unwrap()
            .on_event(&envelope.session_id, &event, now);
        Ok(event)
    }

    /// 現在のスナップショット(人格で飾ったもの)が前回配信と異なれば返す(そして記録する)。
    pub fn changed_snapshot(&self) -> Option<SecretarySnapshot> {
        let now = Instant::now();
        let mut snapshot = self.tracker.lock().unwrap().snapshot(now);
        self.persona.lock().unwrap().decorate(&mut snapshot, now);
        let mut last = self.last_published.lock().unwrap();
        if last.as_ref() == Some(&snapshot) {
            return None;
        }
        *last = Some(snapshot.clone());
        Some(snapshot)
    }

    pub fn expire_stale(&self) -> usize {
        self.tracker.lock().unwrap().expire_stale(Instant::now())
    }

    pub fn set_follow(&self, policy: FollowPolicy) {
        self.tracker.lock().unwrap().set_follow(policy);
    }

    pub fn follow(&self) -> FollowPolicy {
        self.tracker.lock().unwrap().config().follow.clone()
    }
}

/// 変化があれば webview へ配信する。イベント適用後と定期処理から呼ぶ。
pub fn refresh<R: Runtime>(app: &AppHandle<R>, core: &Core) {
    if let Some(snapshot) = core.changed_snapshot() {
        eprintln!(
            "[snapshot] {:?} session={} message={:?}",
            snapshot.status,
            snapshot.session_label.as_deref().unwrap_or("-"),
            snapshot.message.as_deref().unwrap_or("")
        );
        bridge::publish(app, snapshot);
    }
}

/// `Authorization: Bearer <token>` が一致するか。
pub fn authorized(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|presented| presented.trim() == token)
        .unwrap_or(false)
}

/// ログ用の 1 行要約。
fn summarize(env: &HookEnvelope) -> String {
    let session: String = env.session_id.chars().take(8).collect();
    let cwd = env
        .cwd
        .as_deref()
        .and_then(|c| c.trim_end_matches('/').rsplit('/').next())
        .unwrap_or("-");
    let detail = match env.hook_event_name.as_str() {
        "UserPromptSubmit" => {
            let prompt = env.prompt.as_deref().unwrap_or("");
            let origin = secretary_core::split_channel_tag(prompt);
            format!(
                " {}prompt={:?}",
                if origin.from_discord { "discord " } else { "" },
                truncate(&origin.body, 50)
            )
        }
        "PreToolUse" | "PostToolUse" | "PostToolUseFailure" | "PermissionRequest"
        | "PermissionDenied" => {
            format!(" tool={}", env.tool_name.as_deref().unwrap_or("-"))
        }
        "Notification" => format!(" type={}", env.notification_type.as_deref().unwrap_or("-")),
        "Stop" => format!(
            " last={:?}",
            truncate(env.last_assistant_message.as_deref().unwrap_or(""), 40)
        ),
        "SessionEnd" => format!(" reason={}", env.reason.as_deref().unwrap_or("-")),
        _ => String::new(),
    };
    format!(
        "{:<18} session={session} cwd={cwd}{detail}",
        env.hook_event_name
    )
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

#[derive(Clone)]
struct HttpState {
    app: AppHandle,
    core: Arc<Core>,
}

pub fn router(app: AppHandle, core: Arc<Core>) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/hook", post(receive_hook))
        .with_state(HttpState { app, core })
}

async fn receive_hook(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if !authorized(&headers, state.core.token()) {
        eprintln!("[hook] rejected: missing or wrong token");
        return StatusCode::UNAUTHORIZED;
    }
    let text = String::from_utf8_lossy(&body);
    match state.core.ingest(&text) {
        Ok(_) => {
            refresh(&state.app, &state.core);
            StatusCode::NO_CONTENT
        }
        Err(e) => {
            eprintln!("[hook] bad json: {e}");
            StatusCode::BAD_REQUEST
        }
    }
}

/// 127.0.0.1 で待ち受ける。bind に失敗しても(hook-logger と同じポートなど)アプリは動き続ける。
pub async fn serve(app: AppHandle, core: Arc<Core>, port: u16) {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[server] bind {addr} failed: {e} (hook-logger など別プロセスが同じポートを使っていませんか)");
            return;
        }
    };
    eprintln!("[server] listening on http://{addr}");
    if let Err(e) = axum::serve(listener, router(app, core)).await {
        eprintln!("[server] error: {e}");
    }
}

/// 一時状態の期限切れや失効を表示に反映するための定期処理。
pub async fn ticker(app: AppHandle, core: Arc<Core>) {
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    let mut ticks: u32 = 0;
    loop {
        interval.tick().await;
        ticks = ticks.wrapping_add(1);
        if ticks.is_multiple_of(120) {
            let removed = core.expire_stale();
            if removed > 0 {
                eprintln!("[tracker] expired {removed} stale session(s)");
            }
        }
        refresh(&app, &core);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use secretary_core::{AssistantState, TrackerConfig};

    fn core() -> Core {
        Core::new(
            Tracker::new(TrackerConfig {
                follow: FollowPolicy::All,
                ..Default::default()
            }),
            Persona::new(crate::persona::PersonaConfig::default()),
            "secret".into(),
        )
    }

    #[test]
    fn persona_fills_empty_message_and_claude_reply_wins() {
        let core = core();
        core.ingest(
            r#"{"session_id":"s1","hook_event_name":"UserPromptSubmit","cwd":"/x","prompt":"hi"}"#,
        )
        .unwrap();
        let snap = core.changed_snapshot().unwrap();
        assert_eq!(snap.status, AssistantState::Thinking);
        assert!(
            snap.message.is_some(),
            "persona should add a turn-start phrase"
        );
        assert_eq!(
            snap.message_kind,
            Some(secretary_core::SpeechKind::Assistant)
        );

        core.ingest(
            r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/x","tool_name":"mcp__plugin_discord_discord__reply","tool_use_id":"t","tool_input":{"chat_id":"1","text":"やります"}}"#,
        )
        .unwrap();
        let snap = core.changed_snapshot().unwrap();
        assert_eq!(snap.message.as_deref(), Some("やります"));
        assert_eq!(snap.message_kind, Some(secretary_core::SpeechKind::Reply));
    }

    #[test]
    fn bearer_token_is_checked() {
        let mut headers = HeaderMap::new();
        assert!(!authorized(&headers, "secret"));
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer secret"));
        assert!(authorized(&headers, "secret"));
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer wrong"));
        assert!(!authorized(&headers, "secret"));
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Basic secret"));
        assert!(!authorized(&headers, "secret"));
    }

    #[test]
    fn ingest_then_changed_snapshot_only_reports_changes() {
        let core = core();
        assert_eq!(
            core.changed_snapshot().map(|s| s.status),
            Some(AssistantState::Idle)
        );
        assert!(core.changed_snapshot().is_none());
        core.ingest(
            r#"{"session_id":"s1","hook_event_name":"UserPromptSubmit","cwd":"/x","prompt":"hi"}"#,
        )
        .unwrap();
        assert_eq!(
            core.changed_snapshot().map(|s| s.status),
            Some(AssistantState::Thinking)
        );
        assert!(core.changed_snapshot().is_none());
        assert!(core.ingest("not json").is_err());
    }

    #[test]
    fn follow_policy_can_be_switched_at_runtime() {
        let core = core();
        core.ingest(
            r#"{"session_id":"s1","hook_event_name":"UserPromptSubmit","cwd":"/x","prompt":"hi"}"#,
        )
        .unwrap();
        core.set_follow(FollowPolicy::Discord);
        assert_eq!(core.follow(), FollowPolicy::Discord);
        assert_eq!(core.changed_snapshot().map(|s| s.tracked_sessions), Some(0));
    }

    #[test]
    fn summary_marks_discord_prompts() {
        let env = HookEnvelope::parse(
            r#"{"session_id":"abcdefgh-1","hook_event_name":"UserPromptSubmit","cwd":"/a/b","prompt":"<channel source=\"plugin:discord:discord\" x=\"1\">\nやって\n</channel>"}"#,
        )
        .unwrap();
        let line = summarize(&env);
        assert!(line.contains("discord prompt=\"やって\""), "{line}");
        assert!(line.contains("session=abcdefgh cwd=b"));
    }
}
