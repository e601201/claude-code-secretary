//! 設定ファイル(`config.toml`)と hook 認証トークンの読み書き。
//! どちらもアプリの設定ディレクトリ(macOS: ~/Library/Application Support/<identifier>/)に置く。

use std::{fs, io, path::PathBuf, time::Duration};

use secretary_core::{FollowPolicy, HoldConfig, TrackerConfig};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};

pub const CONFIG_FILENAME: &str = "config.toml";
pub const TOKEN_FILENAME: &str = "token";
pub const DEFAULT_PORT: u16 = 47831;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// hook を受け付けるポート(127.0.0.1 のみ)
    pub port: u16,
    /// 追跡方針: "discord"(Discord 由来のセッションだけ) / "all"(全セッション) / 任意の session_id
    pub follow: String,
    /// 空でなければ、cwd がこのいずれかで始まるセッションだけを対象にする
    pub cwd_prefixes: Vec<String>,
    pub success_hold_secs: f64,
    pub error_hold_secs: f64,
    /// この秒数イベントが無いセッションは失効させる
    pub stale_after_secs: u64,
    pub max_message_chars: usize,
}

impl Default for AppConfig {
    fn default() -> Self {
        let hold = HoldConfig::default();
        let tracker = TrackerConfig::default();
        Self {
            port: DEFAULT_PORT,
            follow: "discord".to_string(),
            cwd_prefixes: Vec::new(),
            success_hold_secs: hold.success_hold.as_secs_f64(),
            error_hold_secs: hold.error_hold.as_secs_f64(),
            stale_after_secs: tracker.stale_after.as_secs(),
            max_message_chars: hold.max_message_chars,
        }
    }
}

impl AppConfig {
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    pub fn follow_policy(&self) -> FollowPolicy {
        match self.follow.trim() {
            "all" => FollowPolicy::All,
            "" | "discord" => FollowPolicy::Discord,
            id => FollowPolicy::Session(id.to_string()),
        }
    }

    pub fn tracker_config(&self) -> TrackerConfig {
        TrackerConfig {
            hold: HoldConfig {
                success_hold: Duration::from_secs_f64(self.success_hold_secs.max(0.0)),
                error_hold: Duration::from_secs_f64(self.error_hold_secs.max(0.0)),
                max_message_chars: self.max_message_chars.max(8),
            },
            stale_after: Duration::from_secs(self.stale_after_secs.max(60)),
            follow: self.follow_policy(),
            cwd_prefixes: self.cwd_prefixes.clone(),
        }
    }

    /// 初回に書き出す、コメント付きの設定ファイル。
    pub fn template() -> String {
        let d = Self::default();
        format!(
            "# Claude Code デスクトップ秘書の設定。変更後はアプリを再起動する。\n\
             \n\
             # hook を受け付けるポート(127.0.0.1 のみ)。hook スクリプト側の SECRETARY_PORT と合わせる。\n\
             port = {port}\n\
             \n\
             # 追跡方針: \"discord\" = Discord 由来のセッションだけ / \"all\" = 全セッション / \"<session_id>\" = 固定\n\
             follow = \"{follow}\"\n\
             \n\
             # 空でなければ、cwd がこのいずれかで始まるセッションだけを対象にする\n\
             cwd_prefixes = []\n\
             \n\
             # 一時状態の表示秒数\n\
             success_hold_secs = {success}\n\
             error_hold_secs = {error}\n\
             \n\
             # この秒数イベントが無いセッションは失効させる\n\
             stale_after_secs = {stale}\n\
             \n\
             # 吹き出し文言の最大文字数\n\
             max_message_chars = {chars}\n",
            port = d.port,
            follow = d.follow,
            success = d.success_hold_secs,
            error = d.error_hold_secs,
            stale = d.stale_after_secs,
            chars = d.max_message_chars,
        )
    }
}

pub fn config_dir<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<PathBuf> {
    app.path().app_config_dir()
}

/// 設定を読む。無ければテンプレートを書き出して既定値を返す。壊れていれば既定値で続行する。
pub fn load_or_create<R: Runtime>(app: &AppHandle<R>) -> AppConfig {
    let Ok(dir) = config_dir(app) else {
        eprintln!("[config] app_config_dir unavailable; using defaults");
        return AppConfig::default();
    };
    let path = dir.join(CONFIG_FILENAME);
    match fs::read_to_string(&path) {
        Ok(text) => match AppConfig::parse(&text) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!(
                    "[config] {} is invalid ({e}); using defaults",
                    path.display()
                );
                AppConfig::default()
            }
        },
        Err(_) => {
            if let Err(e) =
                fs::create_dir_all(&dir).and_then(|_| fs::write(&path, AppConfig::template()))
            {
                eprintln!("[config] could not write {}: {e}", path.display());
            } else {
                eprintln!("[config] wrote default config to {}", path.display());
            }
            AppConfig::default()
        }
    }
}

/// hook 認証トークン。無ければ生成して 0600 で保存する。hook スクリプトは同じファイルを読む。
pub fn load_or_create_token<R: Runtime>(app: &AppHandle<R>) -> io::Result<String> {
    let dir = config_dir(app).map_err(|e| io::Error::other(e.to_string()))?;
    let path = dir.join(TOKEN_FILENAME);
    if let Ok(existing) = fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    fs::create_dir_all(&dir)?;
    fs::write(&path, &token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    eprintln!("[config] wrote new hook token to {}", path.display());
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_to_defaults() {
        assert_eq!(
            AppConfig::parse(&AppConfig::template()).unwrap(),
            AppConfig::default()
        );
    }

    #[test]
    fn partial_file_fills_defaults() {
        let cfg = AppConfig::parse("follow = \"all\"\nport = 5000\n").unwrap();
        assert_eq!(cfg.port, 5000);
        assert_eq!(cfg.follow_policy(), FollowPolicy::All);
        assert_eq!(
            cfg.success_hold_secs,
            AppConfig::default().success_hold_secs
        );
    }

    #[test]
    fn follow_policy_variants() {
        assert_eq!(
            AppConfig {
                follow: "discord".into(),
                ..Default::default()
            }
            .follow_policy(),
            FollowPolicy::Discord
        );
        assert_eq!(
            AppConfig {
                follow: "".into(),
                ..Default::default()
            }
            .follow_policy(),
            FollowPolicy::Discord
        );
        assert_eq!(
            AppConfig {
                follow: "abc-123".into(),
                ..Default::default()
            }
            .follow_policy(),
            FollowPolicy::Session("abc-123".into())
        );
    }

    #[test]
    fn tracker_config_clamps_unsafe_values() {
        let cfg = AppConfig {
            stale_after_secs: 1,
            max_message_chars: 0,
            success_hold_secs: -1.0,
            ..Default::default()
        };
        let t = cfg.tracker_config();
        assert_eq!(t.stale_after, Duration::from_secs(60));
        assert_eq!(t.hold.max_message_chars, 8);
        assert_eq!(t.hold.success_hold, Duration::ZERO);
    }
}
