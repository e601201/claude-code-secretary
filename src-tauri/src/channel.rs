//! secretary-channel 子プロセスとの Unix ソケット(Phase 10)。
//!
//! Claude Code のセッションごとに 1 本の接続が来る。最初のフレームは `hello`(session_id と
//! hook と同じトークン)で、以後は [`ChannelEvent`] を受け取り [`ChannelCommand`] を送る。
//! ソケットは設定ディレクトリ直下の `channel.sock`(0600)。

use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use secretary_core::{ChannelCommand, ChannelEvent};
use tauri::AppHandle;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::mpsc,
};

use crate::server::{self, Core};

struct Conn {
    id: u64,
    tx: mpsc::UnboundedSender<ChannelCommand>,
    cwd: Option<String>,
    since: Instant,
}

/// 接続中の channel を session_id で引ける台帳。
#[derive(Default)]
pub struct ChannelHub {
    conns: Mutex<HashMap<String, Conn>>,
    next_id: AtomicU64,
}

impl ChannelHub {
    fn register(&self, session_id: &str, conn: Conn) {
        let replaced = self
            .conns
            .lock()
            .unwrap()
            .insert(session_id.to_string(), conn);
        if replaced.is_some() {
            eprintln!(
                "[channel] session={} reconnected, dropping the older link",
                short(session_id)
            );
        }
    }

    /// この接続がまだ台帳にあるときだけ外す(同じセッションの新しい接続を消さないため)。
    fn unregister(&self, session_id: &str, id: u64) -> bool {
        let mut conns = self.conns.lock().unwrap();
        if conns.get(session_id).map(|c| c.id) == Some(id) {
            conns.remove(session_id);
            true
        } else {
            false
        }
    }

    fn len(&self) -> usize {
        self.conns.lock().unwrap().len()
    }

    pub fn cwd_of(&self, session_id: &str) -> Option<String> {
        self.conns
            .lock()
            .unwrap()
            .get(session_id)
            .and_then(|c| c.cwd.clone())
    }

    /// 送り先を選ぶ。`preferred`(表示中のセッション)が接続中ならそれ、さもなくば最新の接続。
    pub fn pick_target(&self, preferred: Option<&str>) -> Option<String> {
        let conns = self.conns.lock().unwrap();
        if let Some(p) = preferred {
            if conns.contains_key(p) {
                return Some(p.to_string());
            }
        }
        conns
            .iter()
            .max_by_key(|(_, c)| c.since)
            .map(|(id, _)| id.clone())
    }

    pub fn send(&self, session_id: &str, cmd: ChannelCommand) -> Result<(), String> {
        let conns = self.conns.lock().unwrap();
        let conn = conns.get(session_id).ok_or_else(|| {
            format!(
                "セッション {} は秘書につながっていません",
                short(session_id)
            )
        })?;
        conn.tx
            .send(cmd)
            .map_err(|_| "channel への書き込みに失敗しました".to_string())
    }
}

fn short(session_id: &str) -> String {
    session_id.chars().take(8).collect()
}

/// ソケットで待ち受け、接続ごとに [`handle`] を起動する。bind に失敗してもアプリは動き続ける。
pub async fn serve(app: AppHandle, core: Arc<Core>, path: PathBuf) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // 前回の異常終了で残ったファイルを消す(生きている別インスタンスが居れば bind は失敗する)
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[channel] bind {} failed: {e}", path.display());
            return;
        }
    };
    if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
        eprintln!("[channel] chmod {} failed: {e}", path.display());
    }
    eprintln!("[channel] listening on {}", path.display());
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tauri::async_runtime::spawn(handle(app.clone(), core.clone(), stream));
            }
            Err(e) => {
                eprintln!("[channel] accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

async fn handle(app: AppHandle, core: Arc<Core>, stream: UnixStream) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    let Ok(Some(first)) = lines.next_line().await else {
        return;
    };
    let (session_id, cwd, pid) = match serde_json::from_str::<ChannelEvent>(&first) {
        Ok(ChannelEvent::Hello {
            session_id,
            cwd,
            pid,
            token,
        }) => {
            if token.as_deref() != Some(core.token()) {
                eprintln!("[channel] rejected pid {pid}: missing or wrong token");
                return;
            }
            (session_id, cwd, pid)
        }
        Ok(other) => {
            eprintln!("[channel] first frame must be hello, got {other:?}");
            return;
        }
        Err(e) => {
            eprintln!("[channel] bad hello: {e}");
            return;
        }
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<ChannelCommand>();
    let id = core.hub().next_id.fetch_add(1, Ordering::Relaxed);
    core.hub().register(
        &session_id,
        Conn {
            id,
            tx,
            cwd: cwd.clone(),
            since: Instant::now(),
        },
    );
    eprintln!(
        "[channel] connected session={} pid={pid} cwd={} total={}",
        short(&session_id),
        cwd.as_deref().unwrap_or("-"),
        core.hub().len()
    );

    let writer_task = tauri::async_runtime::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            let mut line = serde_json::to_string(&cmd).unwrap_or_default();
            line.push('\n');
            if writer.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    });

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<ChannelEvent>(&line) {
            Ok(event) => {
                core.on_channel_event(&session_id, cwd.as_deref(), &event);
                server::refresh(&app, &core);
            }
            Err(e) => eprintln!(
                "[channel] bad frame from session {}: {e}",
                short(&session_id)
            ),
        }
    }

    writer_task.abort();
    if core.hub().unregister(&session_id, id) {
        core.on_channel_disconnect(&session_id);
        server::refresh(&app, &core);
    }
    eprintln!(
        "[channel] disconnected session={} total={}",
        short(&session_id),
        core.hub().len()
    );
}

/// 秘書の入力欄から。送り先のラベルを返す。
#[tauri::command]
pub fn send_prompt(
    app: AppHandle,
    core: tauri::State<'_, Arc<Core>>,
    text: String,
) -> Result<String, String> {
    let result = core.send_prompt(&text);
    server::refresh(&app, &core);
    result
}

/// 吹き出しの許可 / 拒否ボタンから。
#[tauri::command]
pub fn respond_permission(
    app: AppHandle,
    core: tauri::State<'_, Arc<Core>>,
    session_id: Option<String>,
    request_id: String,
    allow: bool,
) -> Result<(), String> {
    let result = core.respond_permission(session_id.as_deref(), &request_id, allow);
    server::refresh(&app, &core);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(id: u64, since: Instant) -> (Conn, mpsc::UnboundedReceiver<ChannelCommand>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Conn {
                id,
                tx,
                cwd: Some(format!("/work/{id}")),
                since,
            },
            rx,
        )
    }

    #[test]
    fn pick_target_prefers_displayed_session_then_newest() {
        let hub = ChannelHub::default();
        assert_eq!(hub.pick_target(Some("x")), None);
        let now = Instant::now();
        let (a, _ra) = conn(1, now);
        let (b, _rb) = conn(2, now + Duration::from_secs(1));
        hub.register("a", a);
        hub.register("b", b);
        assert_eq!(hub.pick_target(Some("a")).as_deref(), Some("a"));
        assert_eq!(hub.pick_target(Some("zzz")).as_deref(), Some("b"));
        assert_eq!(hub.pick_target(None).as_deref(), Some("b"));
        assert_eq!(hub.cwd_of("a").as_deref(), Some("/work/1"));
    }

    #[test]
    fn send_reaches_the_connection_and_unregister_is_id_checked() {
        let hub = ChannelHub::default();
        let (old, _r_old) = conn(1, Instant::now());
        hub.register("s", old);
        let (new, mut r_new) = conn(2, Instant::now());
        hub.register("s", new);
        // 古い接続の後始末は、新しい接続を消してはいけない
        assert!(!hub.unregister("s", 1));
        assert_eq!(hub.pick_target(Some("s")).as_deref(), Some("s"));
        hub.send("s", ChannelCommand::SendPrompt { text: "hi".into() })
            .unwrap();
        assert_eq!(
            r_new.try_recv().unwrap(),
            ChannelCommand::SendPrompt { text: "hi".into() }
        );
        assert!(hub
            .send("nope", ChannelCommand::SendPrompt { text: "x".into() })
            .is_err());
        assert!(hub.unregister("s", 2));
        assert_eq!(hub.len(), 0);
    }
}
