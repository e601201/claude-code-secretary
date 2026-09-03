//! Phase 0 スパイク用のログサーバー。
//!
//! Claude Code の hook から POST された JSON をそのまま JSONL に追記し、
//! 標準出力に 1 行の要約を出す。Secretary Core の HTTP 受信部の種になる。
//!
//! 環境変数:
//! - `SECRETARY_PORT` 待ち受けポート(既定 47831)
//! - `SECRETARY_LOG`  ログファイル(既定 logs/hooks.jsonl)

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    body::Bytes,
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use tokio::{fs::OpenOptions, io::AsyncWriteExt, sync::Mutex};

struct AppState {
    log_path: PathBuf,
    write_lock: Mutex<()>,
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("SECRETARY_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(47831);
    let log_path = std::env::var("SECRETARY_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("logs/hooks.jsonl"));
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).expect("create log directory");
    }

    let state = Arc::new(AppState {
        log_path: log_path.clone(),
        write_lock: Mutex::new(()),
    });

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/hook", post(receive_hook))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    eprintln!(
        "hook-logger listening on http://{addr}  log={}",
        log_path.display()
    );
    axum::serve(listener, app).await.expect("serve");
}

async fn receive_hook(State(state): State<Arc<AppState>>, body: Bytes) -> StatusCode {
    let now = chrono::Local::now();
    let hook: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => json!({ "raw": String::from_utf8_lossy(&body) }),
    };

    println!("{}", summarize(&now.format("%H:%M:%S%.3f").to_string(), &hook));

    let mut line = json!({ "received_at": now.to_rfc3339(), "hook": hook }).to_string();
    line.push('\n');

    let _guard = state.write_lock.lock().await;
    match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&state.log_path)
        .await
    {
        Ok(mut file) => {
            if let Err(e) = file.write_all(line.as_bytes()).await {
                eprintln!("write error: {e}");
            }
        }
        Err(e) => eprintln!("open error: {e}"),
    }

    StatusCode::NO_CONTENT
}

/// 標準出力向けの 1 行要約。
fn summarize(time: &str, v: &Value) -> String {
    let str_of = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("-");
    let session: String = str_of("session_id").chars().take(8).collect();
    let event = str_of("hook_event_name");
    let cwd = str_of("cwd");

    let detail = match event {
        "UserPromptSubmit" => format!(" prompt={:?}", truncate(str_of("prompt"), 60)),
        "PreToolUse" | "PostToolUse" | "PostToolUseFailure" | "PermissionRequest" => {
            let input = v
                .get("tool_input")
                .map(|i| truncate(&i.to_string(), 80))
                .unwrap_or_default();
            format!(" tool={} input={input}", str_of("tool_name"))
        }
        "SessionStart" => format!(" source={}", str_of("source")),
        "SessionEnd" => format!(" reason={}", str_of("reason")),
        _ => String::new(),
    };

    format!("[{time}] {event:<20} session={session} cwd={cwd}{detail}")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}
