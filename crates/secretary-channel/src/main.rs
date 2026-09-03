//! secretary-channel: 秘書アプリを Claude Code の channel にする stdio MCP サーバー。
//!
//! Claude Code がセッションごとに子プロセスとして起動する(`claude mcp add --scope user secretary`)。
//! 標準入出力で Claude Code と MCP を話し、Unix ソケットで秘書アプリとフレームを交換する。
//!
//! - 秘書アプリが居なくても MCP としては普通に応答し、2 秒ごとに接続を試し続ける
//! - 標準入力が閉じたら(セッション終了)そのまま終了する
//! - 標準出力には JSON-RPC 以外を一切書かない。ログは標準エラー

mod mcp;

use std::{
    env, fs,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use secretary_core::channel::{
    ChannelCommand, ChannelEvent, APP_IDENTIFIER, CHANNEL_SOCKET_ENV, CHANNEL_SOCKET_FILENAME,
    PROJECT_DIR_ENV, SESSION_ID_ENV, TOKEN_FILENAME,
};
use serde_json::Value;

use mcp::{notification_for, Mcp, Outgoing};

const RECONNECT_EVERY: Duration = Duration::from_secs(2);

/// 設定ディレクトリ。Tauri の `app_config_dir()` と同じ場所。
fn config_dir() -> Option<PathBuf> {
    let home = env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library/Application Support")
            .join(APP_IDENTIFIER),
    )
}

fn socket_path() -> Option<PathBuf> {
    if let Some(p) = env::var_os(CHANNEL_SOCKET_ENV) {
        return Some(PathBuf::from(p));
    }
    config_dir().map(|d| d.join(CHANNEL_SOCKET_FILENAME))
}

fn read_token() -> Option<String> {
    let path = config_dir()?.join(TOKEN_FILENAME);
    fs::read_to_string(path)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// 標準出力は 2 つのスレッド(Claude からの要求への応答と、アプリからの指示の転送)から書く。
struct Stdout(Mutex<io::Stdout>);

impl Stdout {
    fn send(&self, msg: &Value) {
        let mut out = self.0.lock().unwrap();
        if writeln!(out, "{msg}").and_then(|_| out.flush()).is_err() {
            // Claude Code 側が閉じた。読み取り側で EOF を検知して終了する
            eprintln!("secretary-channel: stdout closed");
        }
    }
}

/// アプリへの書き込み口。未接続なら None。
struct AppLink {
    writer: Mutex<Option<UnixStream>>,
}

impl AppLink {
    fn send(&self, event: &ChannelEvent) {
        let mut guard = self.writer.lock().unwrap();
        let Some(stream) = guard.as_mut() else {
            eprintln!("secretary-channel: app not connected, dropping {event:?}");
            return;
        };
        let line = serde_json::to_string(event).unwrap_or_default();
        if writeln!(stream, "{line}")
            .and_then(|_| stream.flush())
            .is_err()
        {
            eprintln!("secretary-channel: write to app failed, disconnecting");
            *guard = None;
        }
    }

    fn set(&self, stream: Option<UnixStream>) {
        *self.writer.lock().unwrap() = stream;
    }
}

/// アプリへ接続し、指示を Claude Code へ転送し続ける。切れたら待って繋ぎ直す。
fn app_loop(path: PathBuf, hello: ChannelEvent, link: Arc<AppLink>, stdout: Arc<Stdout>) {
    let mut announced_missing = false;
    loop {
        match UnixStream::connect(&path) {
            Ok(stream) => {
                announced_missing = false;
                match stream.try_clone() {
                    Ok(writer) => link.set(Some(writer)),
                    Err(e) => {
                        eprintln!("secretary-channel: clone socket failed: {e}");
                        thread::sleep(RECONNECT_EVERY);
                        continue;
                    }
                }
                link.send(&hello);
                eprintln!("secretary-channel: connected to {}", path.display());
                let reader = BufReader::new(stream);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<ChannelCommand>(&line) {
                        Ok(cmd) => {
                            eprintln!("secretary-channel: → claude {cmd:?}");
                            stdout.send(&notification_for(&cmd));
                        }
                        Err(e) => eprintln!("secretary-channel: bad frame from app: {e}"),
                    }
                }
                link.set(None);
                eprintln!("secretary-channel: app disconnected");
            }
            Err(_) => {
                if !announced_missing {
                    eprintln!(
                        "secretary-channel: app not running ({}), will keep trying",
                        path.display()
                    );
                    announced_missing = true;
                }
            }
        }
        thread::sleep(RECONNECT_EVERY);
    }
}

fn main() {
    let session_id = env::var(SESSION_ID_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("pid-{}", process::id()));
    let cwd = env::var(PROJECT_DIR_ENV)
        .ok()
        .or_else(|| env::current_dir().ok().map(|p| p.display().to_string()));
    let hello = ChannelEvent::Hello {
        session_id: session_id.clone(),
        cwd,
        pid: process::id(),
        token: read_token(),
    };
    eprintln!("secretary-channel: session={session_id}");

    let stdout = Arc::new(Stdout(Mutex::new(io::stdout())));
    let link = Arc::new(AppLink {
        writer: Mutex::new(None),
    });

    if let Some(path) = socket_path() {
        let (link, stdout) = (link.clone(), stdout.clone());
        thread::spawn(move || app_loop(path, hello, link, stdout));
    } else {
        eprintln!("secretary-channel: HOME not set, running without the app");
    }

    let mut mcp = Mcp::new();
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        for out in mcp.handle_line(&line) {
            match out {
                Outgoing::ToClaude(msg) => stdout.send(&msg),
                Outgoing::ToApp(event) => link.send(&event),
            }
        }
    }
    eprintln!("secretary-channel: stdin closed, exiting");
}
