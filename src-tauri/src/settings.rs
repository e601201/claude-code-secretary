//! 設定画面(Phase 11)。`settings` ウィンドウの webview から呼ばれるコマンド。
//!
//! 保存は `config.toml` を書き直してから Core に反映する。追跡方針・保持時間・表示の大きさ・通知は
//! その場で効き、ポートだけはアプリの再起動が要る(HTTP サーバーの bind をやり直さないため)。

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Runtime};
use ts_rs::TS;

use crate::{
    config::{self, AppConfig},
    persona,
    server::{self, Core},
    sprite::{self, SheetStatus},
};

/// `tauri.conf.json` の windows[].label と一致させる。
pub const SETTINGS_WINDOW: &str = "settings";

/// 設定画面が最初に取りに来る情報。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct SettingsInfo {
    pub config: AppConfig,
    pub config_path: String,
    pub persona_path: String,
    /// 立ち絵のシートが使える状態か。使えない理由もここに入る
    pub sheet: SheetStatus,
    /// ログイン時の自動起動が有効か
    pub autostart_enabled: bool,
    /// 開発版(`tauri dev`)の実行ファイルはビルド版の場所に無いので、自動起動は登録しない
    pub autostart_available: bool,
}

/// 設定ウィンドウを出す(閉じても隠すだけなので、何度でも出せる)。
pub fn open_settings_window<R: Runtime>(app: &AppHandle<R>) {
    crate::show_window(app, SETTINGS_WINDOW);
}

fn autostart_available() -> bool {
    !cfg!(debug_assertions)
}

fn autostart_enabled<R: Runtime>(app: &AppHandle<R>) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
pub fn settings_info(app: AppHandle, core: tauri::State<'_, Arc<Core>>) -> SettingsInfo {
    let dir = config::config_dir(&app).ok();
    let path = |name: &str| {
        dir.as_ref()
            .map(|d| d.join(name).display().to_string())
            .unwrap_or_default()
    };
    SettingsInfo {
        config: core.config(),
        config_path: path(config::CONFIG_FILENAME),
        persona_path: path(persona::PERSONA_FILENAME),
        sheet: sprite::status(&app),
        autostart_enabled: autostart_enabled(&app),
        autostart_available: autostart_available(),
    }
}

/// 保存して反映する。戻り値はアプリの再起動が要るか(ポートが変わったとき)。
#[tauri::command]
pub fn save_config(
    app: AppHandle,
    core: tauri::State<'_, Arc<Core>>,
    config: AppConfig,
) -> Result<bool, String> {
    let config = config.normalized();
    let before = core.config();
    config::save(&app, &config).map_err(|e| format!("設定ファイルを書けません: {e}"))?;
    core.apply_config(config.clone());
    if (config.scale - before.scale).abs() > f64::EPSILON {
        crate::apply_scale(&app, config.scale);
    }
    server::refresh(&app, &core);
    eprintln!("[settings] saved (port {} -> {})", before.port, config.port);
    Ok(config.port != before.port)
}

/// ログイン時の自動起動を切り替える。戻り値は切り替え後の状態。
#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    if !autostart_available() {
        return Err("自動起動はビルド版(bun run tauri build)でのみ設定できます".to_string());
    }
    let launcher = app.autolaunch();
    let result = if enabled {
        launcher.enable()
    } else {
        launcher.disable()
    };
    result.map_err(|e| e.to_string())?;
    Ok(autostart_enabled(&app))
}

/// 設定ファイル、口調の辞書、設定フォルダのいずれかを Finder / 既定のアプリで開く。
#[tauri::command]
pub fn open_path(app: AppHandle, which: String) -> Result<(), String> {
    let dir = config::config_dir(&app).map_err(|e| e.to_string())?;
    let target = match which.as_str() {
        "config" => dir.join(config::CONFIG_FILENAME),
        "persona" => dir.join(persona::PERSONA_FILENAME),
        "dir" => dir,
        other => return Err(format!("unknown target: {other}")),
    };
    std::process::Command::new("open")
        .arg(&target)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("open {} failed: {e}", target.display()))
}

/// `persona.toml` を読み直す。
#[tauri::command]
pub fn reload_persona(app: AppHandle, core: tauri::State<'_, Arc<Core>>) -> Result<(), String> {
    let cfg = persona::load_or_create(&app);
    core.reload_persona(cfg);
    server::refresh(&app, &core);
    eprintln!("[settings] persona reloaded");
    Ok(())
}
