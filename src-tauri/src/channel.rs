//! secretary-channel 子プロセスとの Unix ソケット(Phase 10)。
//!
//! Claude Code のセッションごとに 1 本の接続が来る。最初のフレームは `hello`(session_id と
//! hook と同じトークン)で、以後は [`ChannelEvent`] を受け取り [`ChannelCommand`] を送る。
//! ソケットは設定ディレクトリ直下の `channel.sock`(0600)。

use std::{
    collections::{HashMap, HashSet},
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
    since: Instant,
}

/// 接続中の channel を session_id で引ける台帳。
#[derive(Default)]
pub struct ChannelHub {
    conns: Mutex<HashMap<String, Conn>>,
    next_id: AtomicU64,
}

impl ChannelHub {
    /// 接続を台帳に載せ、その接続の番号を返す(切るときに同じ番号を渡す)。
    pub(crate) fn register(
        &self,
        session_id: &str,
        tx: mpsc::UnboundedSender<ChannelCommand>,
        since: Instant,
    ) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let replaced = self
            .conns
            .lock()
            .unwrap()
            .insert(session_id.to_string(), Conn { id, tx, since });
        if replaced.is_some() {
            eprintln!(
                "[channel] session={} reconnected, dropping the older link",
                short(session_id)
            );
        }
        id
    }

    /// この接続がまだ台帳にあるときだけ外す(同じセッションの新しい接続を消さないため)。
    pub(crate) fn unregister(&self, session_id: &str, id: u64) -> bool {
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

    pub fn is_connected(&self, session_id: &str) -> bool {
        self.conns.lock().unwrap().contains_key(session_id)
    }

    pub fn connected_ids(&self) -> HashSet<String> {
        self.conns.lock().unwrap().keys().cloned().collect()
    }

    /// 話し相手を選ぶ。`on_screen`(吹き出しの主)が接続中ならそれ、さもなくば最新の接続。
    pub fn pick_partner(&self, on_screen: Option<&str>) -> Option<String> {
        let conns = self.conns.lock().unwrap();
        if let Some(p) = on_screen {
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
    let id = core.on_channel_connect(&session_id, cwd.as_deref(), tx);
    eprintln!(
        "[channel] connected session={} pid={pid} cwd={} total={}",
        short(&session_id),
        cwd.as_deref().unwrap_or("-"),
        core.hub().len()
    );
    server::refresh(&app, &core);

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
    if core.on_channel_disconnect(&session_id, id) {
        server::refresh(&app, &core);
    }
    eprintln!(
        "[channel] disconnected session={} total={}",
        short(&session_id),
        core.hub().len()
    );
}

/// webview が入力欄を開いた。この時点で話し相手が決まり、その姿は `refresh` が
/// `TALK_PARTNER_EVENT` で流す(戻り値で返すと、その間に届いた変化を上書きしてしまう)。
#[tauri::command]
pub fn composer_opened(app: AppHandle, core: tauri::State<'_, Arc<Core>>) {
    core.open_composer();
    server::refresh(&app, &core);
}

/// webview が入力欄を閉じた。次に開くときは話し相手を選び直す。
#[tauri::command]
pub fn composer_closed(core: tauri::State<'_, Arc<Core>>) {
    core.close_composer();
}

/// 秘書の入力欄から。`session_id` は入力欄が表示していた話し相手。届いた先のラベルを返す。
#[tauri::command]
pub fn send_prompt(
    app: AppHandle,
    core: tauri::State<'_, Arc<Core>>,
    text: String,
    session_id: Option<String>,
) -> Result<String, String> {
    let result = core.send_prompt(&text, session_id.as_deref());
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

    #[test]
    fn pick_partner_prefers_the_session_on_screen_then_the_newest() {
        let hub = ChannelHub::default();
        assert_eq!(hub.pick_partner(Some("x")), None);
        let now = Instant::now();
        let (ta, _ra) = mpsc::unbounded_channel();
        let (tb, _rb) = mpsc::unbounded_channel();
        hub.register("a", ta, now);
        hub.register("b", tb, now + Duration::from_secs(1));
        assert_eq!(hub.pick_partner(Some("a")).as_deref(), Some("a"));
        assert_eq!(hub.pick_partner(Some("zzz")).as_deref(), Some("b"));
        assert_eq!(hub.pick_partner(None).as_deref(), Some("b"));
        assert!(hub.is_connected("a"));
        assert!(!hub.is_connected("zzz"));
        assert_eq!(hub.connected_ids().len(), 2);
    }

    #[test]
    fn send_reaches_the_connection_and_unregister_is_id_checked() {
        let hub = ChannelHub::default();
        let (t_old, _r_old) = mpsc::unbounded_channel();
        let old = hub.register("s", t_old, Instant::now());
        let (t_new, mut r_new) = mpsc::unbounded_channel();
        let new = hub.register("s", t_new, Instant::now());
        // 古い接続の後始末は、新しい接続を消してはいけない
        assert!(!hub.unregister("s", old));
        assert_eq!(hub.pick_partner(Some("s")).as_deref(), Some("s"));
        hub.send("s", ChannelCommand::SendPrompt { text: "hi".into() })
            .unwrap();
        assert_eq!(
            r_new.try_recv().unwrap(),
            ChannelCommand::SendPrompt { text: "hi".into() }
        );
        assert!(hub
            .send("nope", ChannelCommand::SendPrompt { text: "x".into() })
            .is_err());
        assert!(hub.unregister("s", new));
        assert_eq!(hub.len(), 0);
    }
}
