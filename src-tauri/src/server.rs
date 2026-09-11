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
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Bytes,
    extract::State,
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    routing::{get, post},
    Router,
};
use secretary_core::{
    strip_markdown, AssistantState, ChannelCommand, ChannelEvent, FollowPolicy, HookEnvelope,
    HookEvent, SecretarySnapshot, Tracker,
};
use serde::Serialize;
use tauri::{AppHandle, Runtime};
use ts_rs::TS;

use crate::{
    bridge,
    channel::ChannelHub,
    config::AppConfig,
    notify,
    persona::{Persona, PersonaConfig},
};

/// 送り先の channel が無いときの案内。吹き出しにそのまま出す。
pub const NO_CHANNEL_HINT: &str =
    "秘書につながっているセッションがありません。claude --dangerously-load-development-channels server:secretary で起動してください";

/// 全文ビューが控える本文の上限。実用上まず当たらないが、
/// 病的に長い応答でメモリと描画を潰さないための歯止め。当たったときは黙らず `truncated` で示す。
const FULL_SPEECH_MAX_CHARS: usize = 100_000;

/// 全文ビューが読む、最後の応答本文。
///
/// 吹き出しに出す文言(切り詰め済み)とは別物で、こちらは切らずに控える。
/// ADR-0002 の通りセッションではなく `Core` の持ち物なので、セッションが終わっても残る。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct FullSpeech {
    /// markdown 記号を落とした本文
    pub text: String,
    /// どのセッションのものか(cwd の末尾ディレクトリ名など、人が読めるラベル)
    pub session_label: String,
    /// 受け取った時刻(UNIX epoch ミリ秒)。表示の整形は webview 側で行う
    pub received_at_ms: f64,
    /// 上限に当たって切ったか
    pub truncated: bool,
}

/// 入力欄に出す、話し相手が居ないときの 1 行。
const NO_PARTNER_LINE: &str = "つながっているセッションがありません";

/// 話し相手: 入力欄から話しかけた内容が届くセッション(CONTEXT.md)。
///
/// 入力欄を開いた時点で決まり、閉じるまで変わらない。見せる文言と送ってよいかも
/// ここで決め、webview はそのまま描く。
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct TalkPartner {
    /// 送るときに webview がそのまま `send_prompt` へ渡す。決まっていなければ `None`
    pub session_id: Option<String>,
    /// テキスト欄の上に出す 1 行(「◯◯ へ」など)
    pub line: String,
    /// 送ってよいか。相手が決まっていて、今もつながっているとき
    pub can_send: bool,
}

/// 開いている入力欄。閉じている間は `Core::composer` が `None`。
#[derive(Debug, Clone)]
struct Composer {
    partner: Option<String>,
    on_screen: bool,
}

/// Tracker と人格、最後に配信したスナップショット。
pub struct Core {
    tracker: Mutex<Tracker>,
    persona: Mutex<Persona>,
    token: String,
    last_published: Mutex<Option<SecretarySnapshot>>,
    /// 全文ビュー用の控え。常に 1 件だけで、永続化しない
    last_full_speech: Mutex<Option<FullSpeech>>,
    hub: ChannelHub,
    config: Mutex<AppConfig>,
    /// 開いている入力欄と、その話し相手
    composer: Mutex<Option<Composer>>,
    /// 最後に webview へ配信した話し相手(変化したときだけ配信するため)
    last_published_partner: Mutex<Option<TalkPartner>>,
}

impl Core {
    pub fn new(config: AppConfig, persona: Persona, token: String) -> Self {
        Self {
            tracker: Mutex::new(Tracker::new(config.tracker_config())),
            persona: Mutex::new(persona),
            token,
            last_published: Mutex::new(None),
            last_full_speech: Mutex::new(None),
            hub: ChannelHub::default(),
            config: Mutex::new(config),
            composer: Mutex::new(None),
            last_published_partner: Mutex::new(None),
        }
    }

    pub fn config(&self) -> AppConfig {
        self.config.lock().unwrap().clone()
    }

    /// 設定を差し替える。追跡方針や保持時間はその場で効く。ポートは再起動が要る。
    pub fn apply_config(&self, config: AppConfig) {
        self.tracker
            .lock()
            .unwrap()
            .update_config(config.tracker_config());
        *self.config.lock().unwrap() = config;
    }

    /// 口調の辞書を読み直したときに差し替える。
    pub fn reload_persona(&self, cfg: PersonaConfig) {
        *self.persona.lock().unwrap() = Persona::new(cfg);
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
        if let HookEvent::Stop {
            last_assistant_message: Some(text),
            ..
        } = &event
        {
            self.remember_full_speech(&envelope.session_id, text);
        }
        self.persona
            .lock()
            .unwrap()
            .on_event(&envelope.session_id, &event, now);
        Ok(event)
    }

    /// 全文ビュー用に応答本文を控える。追跡対象のセッションのぶんだけ持つ。
    fn remember_full_speech(&self, session_id: &str, text: &str) {
        if !self.tracker.lock().unwrap().is_session_followed(session_id) {
            return;
        }
        let stripped = strip_markdown(text.trim());
        let stripped = stripped.trim();
        if stripped.is_empty() {
            return;
        }
        let truncated = stripped.chars().count() > FULL_SPEECH_MAX_CHARS;
        let body: String = if truncated {
            stripped.chars().take(FULL_SPEECH_MAX_CHARS).collect()
        } else {
            stripped.to_string()
        };
        // 控えのロックを取る前に組み立てる(session_label が tracker を取りに行くので、
        // 2 つのロックを同時に持たない)
        let message = FullSpeech {
            text: body,
            session_label: self.session_label(session_id),
            received_at_ms: now_epoch_ms(),
            truncated,
        };
        *self.last_full_speech.lock().unwrap() = Some(message);
    }

    /// 全文ビューが読む控え。まだ一度も応答が無ければ `None`。
    pub fn full_speech(&self) -> Option<FullSpeech> {
        self.last_full_speech.lock().unwrap().clone()
    }

    /// 控えがあるか。メニューの有効 / 無効を決めるだけなので、本文は複製しない。
    pub fn has_full_speech(&self) -> bool {
        self.last_full_speech.lock().unwrap().is_some()
    }

    /// 現在のスナップショット(人格で飾ったもの)が前回配信と異なれば返す(そして記録する)。
    #[cfg(test)]
    pub fn changed_snapshot(&self) -> Option<SecretarySnapshot> {
        self.changed_snapshot_with_prev().map(|(_, s)| s)
    }

    /// `changed_snapshot` に加えて、前回配信した状態も返す(通知の判定用)。
    pub fn changed_snapshot_with_prev(
        &self,
    ) -> Option<(Option<AssistantState>, SecretarySnapshot)> {
        let now = Instant::now();
        let mut snapshot = self.tracker.lock().unwrap().snapshot(now);
        self.persona.lock().unwrap().decorate(&mut snapshot, now);
        let mut last = self.last_published.lock().unwrap();
        if last.as_ref() == Some(&snapshot) {
            return None;
        }
        let prev = last.as_ref().map(|s| s.status);
        *last = Some(snapshot.clone());
        Some((prev, snapshot))
    }

    /// 失効したセッションを片付ける。秘書につながっているセッションは、hook が来ていなくても残す。
    pub fn expire_stale(&self) -> usize {
        let connected = self.hub.connected_ids();
        self.tracker
            .lock()
            .unwrap()
            .expire_stale(Instant::now(), |id| connected.contains(id))
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

    /// channel がつながった。hub に載せ(戻り値はその接続の番号)、Tracker に知らせて
    /// ラベルの母集団に入れ、入力欄が相手を待っていればその場で話し相手にする。
    pub fn on_channel_connect(
        &self,
        session_id: &str,
        cwd: Option<&str>,
        tx: tokio::sync::mpsc::UnboundedSender<ChannelCommand>,
    ) -> u64 {
        let now = Instant::now();
        let conn = self.hub.register(session_id, tx, now);
        self.tracker
            .lock()
            .unwrap()
            .note_channel_hello(session_id, cwd, now);
        let on_screen = self.on_screen_session().as_deref() == Some(session_id);
        let mut composer = self.composer.lock().unwrap();
        if let Some(c) = composer.as_mut() {
            if c.partner.is_none() {
                c.partner = Some(session_id.to_string());
                c.on_screen = on_screen;
            }
        }
        conn
    }

    /// channel が切れた。同じセッションの新しい接続が生きていれば何もせず false を返す。
    /// 外れたときは、その接続でしか答えられない権限要求のボタンを消す。
    /// 話し相手は据え置く(切れたことは `TalkPartner` の文言で伝わる)。
    pub fn on_channel_disconnect(&self, session_id: &str, conn: u64) -> bool {
        if !self.hub.unregister(session_id, conn) {
            return false;
        }
        self.tracker
            .lock()
            .unwrap()
            .clear_relayed_permission(session_id);
        true
    }

    /// 入力欄を開いた。この時点で話し相手を決め、閉じるまで変えない。
    /// 決まった姿は `refresh` が `changed_talk_partner` 経由で webview へ流す。
    pub fn open_composer(&self) {
        let (partner, on_screen) = self.pick_partner_now();
        let on_screen = partner.is_some() && partner == on_screen;
        *self.composer.lock().unwrap() = Some(Composer { partner, on_screen });
    }

    pub fn close_composer(&self) {
        *self.composer.lock().unwrap() = None;
        *self.last_published_partner.lock().unwrap() = None;
    }

    /// 今の話し相手。入力欄が閉じていれば `None`。
    pub fn talk_partner(&self) -> Option<TalkPartner> {
        // ラベル取得が tracker のロックを取るので、composer のロックは先に離す
        let composer = self.composer.lock().unwrap().clone()?;
        let Some(id) = composer.partner else {
            return Some(TalkPartner {
                session_id: None,
                line: NO_PARTNER_LINE.to_string(),
                can_send: false,
            });
        };
        let label = self.session_label(&id);
        let connected = self.hub.is_connected(&id);
        let line = if !connected {
            format!("{label} はつながっていません")
        } else if composer.on_screen {
            format!("{label} へ")
        } else {
            format!("{label} へ（画面には出ていません）")
        };
        Some(TalkPartner {
            session_id: Some(id),
            line,
            can_send: connected,
        })
    }

    /// 話し相手が前回配信から変わっていれば返す(そして記録する)。
    pub fn changed_talk_partner(&self) -> Option<TalkPartner> {
        let current = self.talk_partner()?;
        let mut last = self.last_published_partner.lock().unwrap();
        if last.as_ref() == Some(&current) {
            return None;
        }
        *last = Some(current.clone());
        Some(current)
    }

    /// 吹き出しの主(webview に最後に配信したスナップショットのセッション)。
    fn on_screen_session(&self) -> Option<String> {
        self.last_published
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|s| s.session_id.clone())
    }

    /// 今この瞬間の話し相手を選ぶ。吹き出しの主が秘書につながっていればそれ、
    /// いなければ最後につながったセッション。戻り値は (話し相手, 吹き出しの主)。
    fn pick_partner_now(&self) -> (Option<String>, Option<String>) {
        let on_screen = self.on_screen_session();
        let partner = self.hub.pick_partner(on_screen.as_deref());
        (partner, on_screen)
    }

    /// 秘書からの指示を送る。届いた先のラベルを返す。
    ///
    /// `session_id` は入力欄が表示していた話し相手。無い経路(`POST /say`)は
    /// その場で選ぶ。
    pub fn send_prompt(&self, text: &str, session_id: Option<&str>) -> Result<String, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("空の指示は送れません".to_string());
        }
        let partner = match session_id {
            Some(id) => {
                if !self.hub.is_connected(id) {
                    return Err(format!("{} はつながっていません", self.session_label(id)));
                }
                id.to_string()
            }
            None => self
                .pick_partner_now()
                .0
                .ok_or_else(|| NO_CHANNEL_HINT.to_string())?,
        };
        self.hub.send(
            &partner,
            ChannelCommand::SendPrompt {
                text: text.to_string(),
            },
        )?;
        eprintln!(
            "[channel] send_prompt session={} text={:?}",
            short(&partner),
            truncate(text, 50)
        );
        Ok(self.session_label(&partner))
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

    /// 人が読めるラベル。規則は `Tracker::label_of`(同名のセッションがあれば id の先頭を添える)。
    fn session_label(&self, session_id: &str) -> String {
        self.tracker
            .lock()
            .unwrap()
            .label_of(session_id)
            .unwrap_or_else(|| short(session_id))
    }
}

/// 壁時計を UNIX epoch ミリ秒で。`secretary-core` は Instant しか扱わない契約なので、
/// 絶対時刻が要る全文ビューのぶんだけここで取る。
fn now_epoch_ms() -> f64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as f64,
        Err(e) => {
            eprintln!("[fulltext] clock is before the epoch: {e}");
            0.0
        }
    }
}

fn short(session_id: &str) -> String {
    session_id.chars().take(8).collect()
}

/// 変化があれば webview へ配信する。イベント適用後と定期処理から呼ぶ。
pub fn refresh<R: Runtime>(app: &AppHandle<R>, core: &Core) {
    // 控えは吹き出しの主とは無関係に増えるので、スナップショットの変化とは切り離して合わせる。
    // ここを publish の中に置くと、別セッションが吹き出しを占めている間に控えができたとき、
    // トレイの項目が無効のまま残って全文ビューに到達できなくなる(ADR-0001)。
    crate::update_tray_fulltext_item(app, core.has_full_speech());
    // 話し相手は入力欄が開いている間だけ意味を持ち、接続の増減で変わる
    if let Some(partner) = core.changed_talk_partner() {
        eprintln!(
            "[composer] partner={:?} can_send={}",
            partner.line, partner.can_send
        );
        bridge::publish_talk_partner(app, &partner);
    }
    if let Some((prev, snapshot)) = core.changed_snapshot_with_prev() {
        eprintln!(
            "[snapshot] {:?} session={} message={:?}",
            snapshot.status,
            snapshot.session_label.as_deref().unwrap_or("-"),
            snapshot.message.as_deref().unwrap_or("")
        );
        if core.config().notify_on_waiting {
            notify::on_transition(app, prev, &snapshot);
        }
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
    let result = state.core.send_prompt(&text, None);
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

    fn core() -> Core {
        Core::new(
            AppConfig {
                follow: "all".into(),
                ..Default::default()
            },
            Persona::new(crate::persona::PersonaConfig::default()),
            "secret".into(),
        )
    }

    /// 既定(channel 由来のセッションだけを追跡)の Core。
    fn core_channel_only() -> Core {
        Core::new(
            AppConfig::default(),
            Persona::new(crate::persona::PersonaConfig::default()),
            "secret".into(),
        )
    }

    fn prompt_body(session: &str, cwd: &str) -> String {
        serde_json::json!({
            "session_id": session,
            "hook_event_name": "UserPromptSubmit",
            "cwd": cwd,
            "prompt": "hi",
        })
        .to_string()
    }

    /// 秘書 channel がつながった(`channel::handle` と同じ入口)。届いたコマンドは返り値で読める。
    fn connect(
        core: &Core,
        session: &str,
        cwd: &str,
    ) -> (u64, tokio::sync::mpsc::UnboundedReceiver<ChannelCommand>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = core.on_channel_connect(session, Some(cwd), tx);
        (conn, rx)
    }

    fn stop_body(session: &str, message: &str) -> String {
        serde_json::json!({
            "session_id": session,
            "hook_event_name": "Stop",
            "cwd": "/Users/me/workspace/myproject",
            "last_assistant_message": message,
        })
        .to_string()
    }

    #[test]
    fn stop_remembers_the_full_assistant_message() {
        let core = core();
        let long = "あ".repeat(500);
        core.ingest(&stop_body("s1", &format!("**見出し**\n{long}")))
            .unwrap();
        let full = core.full_speech().expect("控えられているはず");
        // 吹き出しは 120 字で切るが、全文ビューは切らない
        assert_eq!(full.text, format!("見出し\n{long}"));
        assert!(!full.truncated);
        assert_eq!(full.session_label, "myproject");
        assert!(full.received_at_ms > 0.0);
    }

    #[test]
    fn full_speech_is_not_remembered_for_unfollowed_sessions() {
        let core = core_channel_only();
        core.ingest(&stop_body("plain", "ターミナルで直接動かしたぶん"))
            .unwrap();
        assert!(core.full_speech().is_none());
    }

    #[test]
    fn full_speech_survives_session_end() {
        // ADR-0002: SessionState は SessionEnd で捨てられるので、全文は Core に持つ
        let core = core();
        core.ingest(&stop_body("s1", "読み返したい長い応答"))
            .unwrap();
        core.ingest(
            &serde_json::json!({"session_id":"s1","hook_event_name":"SessionEnd"}).to_string(),
        )
        .unwrap();
        assert_eq!(
            core.full_speech().map(|f| f.text),
            Some("読み返したい長い応答".to_string())
        );
    }

    #[test]
    fn a_newer_assistant_message_replaces_the_previous_one() {
        let core = core();
        core.ingest(&stop_body("s1", "ふるい")).unwrap();
        core.ingest(&stop_body("s2", "あたらしい")).unwrap();
        assert_eq!(
            core.full_speech().map(|f| f.text),
            Some("あたらしい".to_string())
        );
    }

    #[test]
    fn absurdly_long_messages_are_capped_and_say_so() {
        let core = core();
        core.ingest(&stop_body("s1", &"x".repeat(FULL_SPEECH_MAX_CHARS + 10)))
            .unwrap();
        let full = core.full_speech().unwrap();
        assert_eq!(full.text.chars().count(), FULL_SPEECH_MAX_CHARS);
        assert!(full.truncated, "黙って切らず、切ったことを見せる");
    }

    #[test]
    fn apply_config_switches_follow_and_reports_previous_status() {
        let core = core();
        core.ingest(
            r#"{"session_id":"s1","hook_event_name":"UserPromptSubmit","cwd":"/x","prompt":"hi"}"#,
        )
        .unwrap();
        let (prev, snap) = core.changed_snapshot_with_prev().unwrap();
        assert_eq!(prev, None);
        assert_eq!(snap.status, AssistantState::Thinking);
        core.apply_config(AppConfig {
            follow: "discord".into(),
            ..Default::default()
        });
        assert_eq!(core.follow(), FollowPolicy::Channel);
        let (prev, snap) = core.changed_snapshot_with_prev().unwrap();
        assert_eq!(prev, Some(AssistantState::Thinking));
        assert_eq!(snap.tracked_sessions, 0);
        assert_eq!(core.config().follow, "discord");
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
            core.send_prompt("  ", None),
            Err("空の指示は送れません".to_string())
        );
        assert_eq!(
            core.send_prompt("直して", None),
            Err(NO_CHANNEL_HINT.to_string())
        );
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
        let (conn, _rx) = connect(&core, "s1", "/x");
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
        assert!(core.on_channel_disconnect("s1", conn));
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

    #[test]
    fn composer_talks_to_the_session_on_screen() {
        let core = core();
        core.ingest(&prompt_body("s1", "/w/app")).unwrap();
        let _c = connect(&core, "s1", "/w/app");
        core.changed_snapshot().unwrap();
        core.open_composer();
        let partner = core.talk_partner().unwrap();
        assert_eq!(partner.session_id.as_deref(), Some("s1"));
        assert_eq!(partner.line, "app へ");
        assert!(partner.can_send);
    }

    #[test]
    fn composer_falls_back_to_the_newest_connection_when_the_screen_session_is_not_connected() {
        let core = core();
        // 画面に出ているのは s1 だが、秘書につながっているのは s2 だけ
        core.ingest(&prompt_body("s1", "/w/app")).unwrap();
        core.changed_snapshot().unwrap();
        let _c = connect(&core, "s2", "/w/lib");
        core.open_composer();
        let partner = core.talk_partner().unwrap();
        assert_eq!(partner.session_id.as_deref(), Some("s2"));
        assert_eq!(partner.line, "lib へ（画面には出ていません）");
        assert!(partner.can_send);
    }

    #[test]
    fn composer_without_anyone_connected_takes_the_first_session_to_connect_and_keeps_it() {
        let core = core();
        core.open_composer();
        let partner = core.talk_partner().unwrap();
        assert_eq!(partner.session_id, None);
        assert_eq!(partner.line, "つながっているセッションがありません");
        assert!(!partner.can_send);
        assert_eq!(
            core.send_prompt("直して", None),
            Err(NO_CHANNEL_HINT.to_string())
        );

        let (_, mut rx1) = connect(&core, "s1", "/w/app");
        let _c2 = connect(&core, "s2", "/w/lib");
        let partner = core.talk_partner().unwrap();
        assert_eq!(partner.session_id.as_deref(), Some("s1"));
        assert_eq!(partner.line, "app へ（画面には出ていません）");
        assert!(partner.can_send);

        assert_eq!(
            core.send_prompt("直して", partner.session_id.as_deref()),
            Ok("app".to_string())
        );
        assert_eq!(
            rx1.try_recv().unwrap(),
            ChannelCommand::SendPrompt {
                text: "直して".into()
            }
        );
    }

    #[test]
    fn talk_partner_stays_put_while_the_bubble_owner_changes() {
        let core = core();
        core.ingest(&prompt_body("s1", "/w/app")).unwrap();
        let (_, mut rx1) = connect(&core, "s1", "/w/app");
        let (_, mut rx2) = connect(&core, "s2", "/w/lib");
        core.changed_snapshot().unwrap();
        core.open_composer();
        let partner = core.talk_partner().unwrap();
        assert_eq!(partner.session_id.as_deref(), Some("s1"));

        // 打っている間に s2 が吹き出しを奪う
        core.ingest(&prompt_body("s2", "/w/lib")).unwrap();
        let snap = core.changed_snapshot().unwrap();
        assert_eq!(snap.session_id.as_deref(), Some("s2"));
        assert_eq!(core.talk_partner().unwrap(), partner);

        assert_eq!(
            core.send_prompt("直して", partner.session_id.as_deref()),
            Ok("app".to_string())
        );
        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_err());

        // 閉じて開き直せば、今の吹き出しの主が話し相手になる
        core.close_composer();
        assert_eq!(core.talk_partner(), None);
        core.open_composer();
        assert_eq!(
            core.talk_partner().unwrap().session_id.as_deref(),
            Some("s2")
        );
    }

    #[test]
    fn disconnected_partner_is_kept_but_cannot_be_sent_to() {
        let core = core();
        core.ingest(&prompt_body("s1", "/w/app")).unwrap();
        let (c1, _rx1) = connect(&core, "s1", "/w/app");
        let _c2 = connect(&core, "s2", "/w/lib");
        core.changed_snapshot().unwrap();
        core.open_composer();
        assert_eq!(
            core.talk_partner().unwrap().session_id.as_deref(),
            Some("s1")
        );

        assert!(core.on_channel_disconnect("s1", c1));
        let partner = core.talk_partner().unwrap();
        assert_eq!(partner.session_id.as_deref(), Some("s1"));
        assert_eq!(partner.line, "app はつながっていません");
        assert!(!partner.can_send);
        assert_eq!(
            core.send_prompt("直して", Some("s1")),
            Err("app はつながっていません".to_string())
        );

        // 入力欄を介さない経路(POST /say)は、その場で残っている接続を選ぶ
        core.close_composer();
        assert_eq!(core.send_prompt("直して", None), Ok("lib".to_string()));
    }

    #[test]
    fn a_stale_disconnect_of_a_reconnected_session_is_ignored() {
        let core = core();
        let (old, _rx_old) = connect(&core, "s1", "/w/app");
        let (_new, _rx_new) = connect(&core, "s1", "/w/app");
        // 古い接続の後始末は、新しい接続を外してはいけない
        assert!(!core.on_channel_disconnect("s1", old));
        assert!(core.hub().is_connected("s1"));
    }

    #[test]
    fn changed_talk_partner_reports_only_changes_while_the_composer_is_open() {
        let core = core();
        assert_eq!(core.changed_talk_partner(), None);
        core.open_composer();
        // 開いた直後の姿も、接続の増減による変化も、同じ経路で 1 回ずつ流れる
        let opened = core.changed_talk_partner().unwrap();
        assert_eq!(opened.session_id, None);
        assert_eq!(core.changed_talk_partner(), None);

        let (c1, _rx) = connect(&core, "s1", "/w/app");
        assert_eq!(
            core.changed_talk_partner().unwrap().session_id.as_deref(),
            Some("s1")
        );
        assert_eq!(core.changed_talk_partner(), None);

        assert!(core.on_channel_disconnect("s1", c1));
        assert!(!core.changed_talk_partner().unwrap().can_send);

        core.close_composer();
        assert_eq!(core.changed_talk_partner(), None);
    }
}
