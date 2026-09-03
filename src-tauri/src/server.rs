//! hook を受け取る HTTP サーバーと、Tracker を包む Core。
//!
//! - `POST /hook`       hook の JSON をそのまま受け取る。`Authorization: Bearer <token>` 必須
//! - `GET  /health`     稼働確認
//! - `POST /say`        本文をそのまま秘書からの指示として channel へ送る(Phase 10、要トークン)
//! - `POST /permission` `{"request_id":"…","allow":true}` で中継された権限要求に答える(要トークン)
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
use secretary_core::{
    ChannelCommand, ChannelEvent, FollowPolicy, HookEnvelope, HookEvent, SecretarySnapshot, Tracker,
};
use tauri::{AppHandle, Runtime};

use crate::{bridge, channel::ChannelHub, persona::Persona};

/// 送り先の channel が無いときの案内。吹き出しにそのまま出す。
pub const NO_CHANNEL_HINT: &str =
    "秘書につながっているセッションがありません。claude --dangerously-load-development-channels server:secretary で起動してください";

/// Tracker と人格、最後に配信したスナップショット。
pub struct Core {
    tracker: Mutex<Tracker>,
    persona: Mutex<Persona>,
    token: String,
    last_published: Mutex<Option<SecretarySnapshot>>,
    hub: ChannelHub,
}

impl Core {
    pub fn new(tracker: Tracker, persona: Persona, token: String) -> Self {
        Self {
            tracker: Mutex::new(tracker),
            persona: Mutex::new(persona),
            token,
            last_published: Mutex::new(None),
            hub: ChannelHub::default(),
        }
    }

    pub fn hub(&self) -> &ChannelHub {
        &self.hub
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

    /// channel 子プロセスからのフレームを反映する。
    pub fn on_channel_event(&self, session_id: &str, cwd: Option<&str>, event: &ChannelEvent) {
        let now = Instant::now();
        match event {
            ChannelEvent::Reply { text } => {
                eprintln!(
                    "[channel] reply session={} text={:?}",
                    short(session_id),
                    truncate(text, 50)
                );
                self.tracker
                    .lock()
                    .unwrap()
                    .note_reply(session_id, cwd, text, now);
            }
            ChannelEvent::PermissionRequest(permission) => {
                eprintln!(
                    "[channel] permission_request session={} id={} tool={} desc={:?}",
                    short(session_id),
                    permission.request_id,
                    permission.tool_name,
                    truncate(&permission.description, 50)
                );
                self.tracker.lock().unwrap().note_relayed_permission(
                    session_id,
                    cwd,
                    permission.clone(),
                    now,
                );
            }
            ChannelEvent::Hello { .. } => {}
        }
    }

    /// channel が切れたら、その接続でしか答えられない権限要求のボタンを消す。
    pub fn on_channel_disconnect(&self, session_id: &str) {
        self.tracker
            .lock()
            .unwrap()
            .clear_relayed_permission(session_id);
    }

    /// 秘書からの指示を送る。送り先は表示中のセッション、無ければ最新の接続。ラベルを返す。
    pub fn send_prompt(&self, text: &str) -> Result<String, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("空の指示は送れません".to_string());
        }
        let preferred = self
            .last_published
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|s| s.session_id.clone());
        let target = self
            .hub
            .pick_target(preferred.as_deref())
            .ok_or_else(|| NO_CHANNEL_HINT.to_string())?;
        self.hub.send(
            &target,
            ChannelCommand::SendPrompt {
                text: text.to_string(),
            },
        )?;
        eprintln!(
            "[channel] send_prompt session={} text={:?}",
            short(&target),
            truncate(text, 50)
        );
        Ok(self.session_label(&target))
    }

    /// 中継された権限要求に答える。session_id が無ければ request_id から探す。
    pub fn respond_permission(
        &self,
        session_id: Option<&str>,
        request_id: &str,
        allow: bool,
    ) -> Result<(), String> {
        let session = match session_id {
            Some(s) => s.to_string(),
            None => self
                .tracker
                .lock()
                .unwrap()
                .sessions()
                .find(|s| {
                    s.relayed_permission()
                        .map(|p| p.request_id.as_str() == request_id)
                        .unwrap_or(false)
                })
                .map(|s| s.id().to_string())
                .ok_or_else(|| format!("権限要求 {request_id} は見つかりません"))?,
        };
        self.hub.send(
            &session,
            ChannelCommand::RespondPermission {
                request_id: request_id.to_string(),
                allow,
            },
        )?;
        self.tracker
            .lock()
            .unwrap()
            .resolve_relayed_permission(&session);
        eprintln!(
            "[channel] permission {} id={request_id} session={}",
            if allow { "allow" } else { "deny" },
            short(&session)
        );
        Ok(())
    }

    fn session_label(&self, session_id: &str) -> String {
        if let Some(s) = self.tracker.lock().unwrap().session(session_id) {
            return s.label();
        }
        self.hub
            .cwd_of(session_id)
            .as_deref()
            .and_then(|c| {
                c.trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .map(str::to_string)
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| short(session_id))
    }
}

fn short(session_id: &str) -> String {
    session_id.chars().take(8).collect()
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
            let via = if origin.from_discord() {
                "discord "
            } else if origin.from_secretary() {
                "secretary "
            } else {
                ""
            };
            format!(" {via}prompt={:?}", truncate(&origin.body, 50))
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
        .route("/say", post(say))
        .route("/permission", post(permission))
        .with_state(HttpState { app, core })
}

/// 本文をそのまま指示として送る。スクリプト(`scripts/say.sh`)から使う。
async fn say(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, String) {
    if !authorized(&headers, state.core.token()) {
        return (StatusCode::UNAUTHORIZED, String::new());
    }
    let text = String::from_utf8_lossy(&body);
    let result = state.core.send_prompt(&text);
    refresh(&state.app, &state.core);
    match result {
        Ok(label) => (StatusCode::OK, label),
        Err(e) => (StatusCode::CONFLICT, e),
    }
}

#[derive(serde::Deserialize)]
struct PermissionBody {
    request_id: String,
    allow: bool,
    #[serde(default)]
    session_id: Option<String>,
}

async fn permission(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, String) {
    if !authorized(&headers, state.core.token()) {
        return (StatusCode::UNAUTHORIZED, String::new());
    }
    let parsed: PermissionBody = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()),
    };
    let result = state.core.respond_permission(
        parsed.session_id.as_deref(),
        &parsed.request_id,
        parsed.allow,
    );
    refresh(&state.app, &state.core);
    match result {
        Ok(()) => (StatusCode::NO_CONTENT, String::new()),
        Err(e) => (StatusCode::CONFLICT, e),
    }
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
        core.set_follow(FollowPolicy::Channel);
        assert_eq!(core.follow(), FollowPolicy::Channel);
        assert_eq!(core.changed_snapshot().map(|s| s.tracked_sessions), Some(0));
    }

    #[test]
    fn send_prompt_without_channel_explains_how_to_start_one() {
        let core = core();
        assert_eq!(
            core.send_prompt("  "),
            Err("空の指示は送れません".to_string())
        );
        assert_eq!(core.send_prompt("直して"), Err(NO_CHANNEL_HINT.to_string()));
        assert!(core.respond_permission(None, "abcde", true).is_err());
    }

    #[test]
    fn relayed_permission_shows_up_in_snapshot_and_disconnect_clears_it() {
        let core = core();
        let perm = secretary_core::RelayedPermission {
            request_id: "abcde".into(),
            tool_name: "Bash".into(),
            description: "Run tests".into(),
            input_preview: "{}".into(),
        };
        core.on_channel_event(
            "s1",
            Some("/x"),
            &ChannelEvent::PermissionRequest(perm.clone()),
        );
        let snap = core.changed_snapshot().unwrap();
        assert_eq!(snap.status, AssistantState::Waiting);
        assert_eq!(snap.relayed_permission, Some(perm));
        assert_eq!(
            snap.message_kind,
            Some(secretary_core::SpeechKind::Permission)
        );
        core.on_channel_disconnect("s1");
        let snap = core.changed_snapshot().unwrap();
        assert_eq!(snap.relayed_permission, None);
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
