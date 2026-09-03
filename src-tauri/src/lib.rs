//! デスクトップ秘書のアプリ本体。
//!
//! Phase 2 の範囲: 透明で装飾のない常時前面ウィンドウにキャラクターを表示し、
//! Dock には出さずメニューバーのトレイから操作する。ウィンドウ位置は再起動後も保つ。

use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{TrayIcon, TrayIconBuilder},
    AppHandle, LogicalPosition, LogicalSize, Manager, Monitor, RunEvent, Runtime, WebviewWindow,
};

/// `tauri.conf.json` の windows[].label と一致させる。
pub const CHARACTER_WINDOW: &str = "character";

const TRAY_ID: &str = "main";
const MENU_CLICK_THROUGH: &str = "click_through";
const MENU_ALWAYS_ON_TOP: &str = "always_on_top";
const MENU_QUIT: &str = "quit";

/// ウィンドウ位置の保存ファイル名(app_config_dir 直下)。
const STATE_FILENAME: &str = "window-state.json";
/// 初回配置のときの画面端からの余白(論理ピクセル)。
const EDGE_MARGIN: f64 = 24.0;

/// トレイは最後のハンドルが破棄されると消える参照カウント方式なので、アプリ状態で保持する。
struct TrayHandle<R: Runtime>(#[allow(dead_code)] TrayIcon<R>);

/// 保存する位置(論理ピクセル)。
///
/// 物理ピクセルは「論理 × その時点の倍率」で作られ、非表示のウィンドウは倍率が 1.0 と
/// 報告されるため、物理で保存すると Retina で 2 倍の位置に復元されてしまう。
/// 論理で保存し `LogicalPosition` で復元すれば倍率に依存しない。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct SavedPosition {
    x: f64,
    y: f64,
}

/// 保存ファイルの形式。`version` が一致しないファイルは無視する。
#[derive(Debug, Default, Serialize, Deserialize)]
struct StateFile {
    #[serde(default)]
    version: u32,
    character: Option<SavedPosition>,
}
const STATE_VERSION: u32 = 2;

/// 論理ピクセルの矩形。ディスプレイや作業領域の判定に使う。
#[derive(Debug, Clone, Copy)]
struct LogicalRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl LogicalRect {
    fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.width && py >= self.y && py < self.y + self.height
    }
}

fn monitor_rect(m: &Monitor) -> LogicalRect {
    let scale = m.scale_factor();
    let p = m.position().to_logical::<f64>(scale);
    let z = m.size().to_logical::<f64>(scale);
    LogicalRect {
        x: p.x,
        y: p.y,
        width: z.width,
        height: z.height,
    }
}

/// Dock やメニューバーを除いた作業領域。
fn monitor_work_rect(m: &Monitor) -> LogicalRect {
    let scale = m.scale_factor();
    let a = m.work_area();
    LogicalRect {
        x: a.position.x as f64 / scale,
        y: a.position.y as f64 / scale,
        width: a.size.width as f64 / scale,
        height: a.size.height as f64 / scale,
    }
}

/// ウィンドウの論理座標。物理値を同じ倍率で割り戻すので、非表示でも正しい。
fn window_logical_position<R: Runtime>(
    window: &WebviewWindow<R>,
) -> tauri::Result<LogicalPosition<f64>> {
    let scale = window.scale_factor()?;
    Ok(window.outer_position()?.to_logical(scale))
}

fn window_logical_size<R: Runtime>(window: &WebviewWindow<R>) -> tauri::Result<LogicalSize<f64>> {
    let scale = window.scale_factor()?;
    Ok(window.outer_size()?.to_logical(scale))
}

fn character_window<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window(CHARACTER_WINDOW)
}

fn state_path<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join(STATE_FILENAME))
}

fn load_saved_position<R: Runtime>(app: &AppHandle<R>) -> Option<SavedPosition> {
    let path = state_path(app)?;
    let text = fs::read_to_string(path).ok()?;
    let state: StateFile = serde_json::from_str(&text).ok()?;
    (state.version == STATE_VERSION)
        .then_some(state.character)
        .flatten()
}

/// 現在のウィンドウ位置を保存する。終了時とトレイの「終了」から呼ぶ。
fn save_position<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let (Some(window), Some(path)) = (character_window(app), state_path(app)) else {
        return Ok(());
    };
    let position = window_logical_position(&window)?;
    let state = StateFile {
        version: STATE_VERSION,
        character: Some(SavedPosition {
            x: position.x,
            y: position.y,
        }),
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, serde_json::to_string_pretty(&state)?)?;
    Ok(())
}

/// カーソルのあるディスプレイの右下(Dock を避けた作業領域内)に置く。
fn place_on_cursor_monitor<R: Runtime>(window: &WebviewWindow<R>) -> tauri::Result<()> {
    let monitors = window.available_monitors()?;
    let primary = window.primary_monitor()?;
    // cursor_position はメインディスプレイの倍率で物理化されている(tao の実装)
    let primary_scale = primary.as_ref().map(|m| m.scale_factor()).unwrap_or(1.0);
    let cursor = window.cursor_position()?.to_logical::<f64>(primary_scale);
    let target = monitors
        .into_iter()
        .find(|m| monitor_rect(m).contains(cursor.x, cursor.y))
        .or(primary);
    let Some(monitor) = target else {
        return Ok(());
    };
    let area = monitor_work_rect(&monitor);
    let size = window_logical_size(window)?;
    let x = area.x + area.width - size.width - EDGE_MARGIN;
    let y = area.y + area.height - size.height - EDGE_MARGIN;
    window.set_position(LogicalPosition::new(x, y))
}

/// 保存済みの位置へ戻し、無いか画面外なら初回配置をしてから表示する。
fn restore_and_show<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let Some(window) = character_window(app) else {
        return Ok(());
    };
    let size = window_logical_size(&window)?;
    let monitors = window.available_monitors()?;
    let restored = match load_saved_position(app) {
        Some(p) => {
            let (cx, cy) = (p.x + size.width / 2.0, p.y + size.height / 2.0);
            let visible = monitors.iter().any(|m| monitor_rect(m).contains(cx, cy));
            if visible {
                window.set_position(LogicalPosition::new(p.x, p.y))?;
            }
            visible
        }
        None => false,
    };
    if !restored {
        place_on_cursor_monitor(&window)?;
    }
    window.show()
}

/// メニューバーのトレイアイコンとメニューを作る。
fn build_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let click_through = CheckMenuItem::with_id(
        app,
        MENU_CLICK_THROUGH,
        "クリック透過",
        true,
        false,
        None::<&str>,
    )?;
    let always_on_top = CheckMenuItem::with_id(
        app,
        MENU_ALWAYS_ON_TOP,
        "常に前面に表示",
        true,
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "終了", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &click_through,
            &always_on_top,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    // アイコン無しで作ると macOS では幅ゼロの項目になって見えないので、必ず埋め込み画像を使う。
    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .icon(tauri::include_image!("icons/32x32.png"))
        .icon_as_template(false)
        .menu(&menu)
        .tooltip("Secretary")
        .show_menu_on_left_click(true);
    // 開発中はアイコンの隣に文字も出して、メニューバー上で見つけやすくする。
    if cfg!(debug_assertions) {
        tray = tray.title("秘書");
    }

    let tray = tray
        .on_menu_event(move |app, event| match event.id().as_ref() {
            MENU_CLICK_THROUGH => {
                let on = click_through.is_checked().unwrap_or(false);
                if let Some(window) = character_window(app) {
                    if let Err(e) = window.set_ignore_cursor_events(on) {
                        eprintln!("set_ignore_cursor_events failed: {e}");
                    }
                }
            }
            MENU_ALWAYS_ON_TOP => {
                let on = always_on_top.is_checked().unwrap_or(true);
                if let Some(window) = character_window(app) {
                    if let Err(e) = window.set_always_on_top(on) {
                        eprintln!("set_always_on_top failed: {e}");
                    }
                }
            }
            MENU_QUIT => {
                if let Err(e) = save_position(app) {
                    eprintln!("save_position failed: {e}");
                }
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;
    app.manage(TrayHandle(tray));

    Ok(())
}

/// フロントエンドからの診断ログ。WebView の様子をターミナル側で確認するために使う。
#[tauri::command]
fn frontend_log(message: String) {
    eprintln!("[frontend] {message}");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            // Dock にアイコンを出さない(macOS)。skipTaskbar は Windows / Linux 専用。
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            build_tray(app.handle())?;
            restore_and_show(app.handle())?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![frontend_log])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app, event| {
        if let RunEvent::ExitRequested { .. } = event {
            if let Err(e) = save_position(app) {
                eprintln!("save_position failed: {e}");
            }
        }
    });
}
