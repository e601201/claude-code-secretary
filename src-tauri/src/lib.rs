//! デスクトップ秘書のアプリ本体。
//!
//! Phase 2 の範囲: 透明で装飾のない常時前面ウィンドウにキャラクターを表示し、
//! Dock には出さずメニューバーのトレイから操作する。ウィンドウ位置は再起動後も保つ。

mod bridge;
mod channel;
mod config;
mod notify;
mod persona;
mod server;
mod settings;

use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tauri::{
    menu::{CheckMenuItem, ContextMenu, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{TrayIcon, TrayIconBuilder},
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Monitor, RunEvent, Runtime,
    WebviewWindow,
};

use bridge::SnapshotBridge;
use secretary_core::{
    channel::CHANNEL_SOCKET_FILENAME, AssistantState, FollowPolicy, SecretarySnapshot,
};
use server::Core;

/// `tauri.conf.json` の windows[].label と一致させる。
pub const CHARACTER_WINDOW: &str = "character";

const TRAY_ID: &str = "main";
const MENU_CLICK_THROUGH: &str = "click_through";
const MENU_ALWAYS_ON_TOP: &str = "always_on_top";
const MENU_QUIT: &str = "quit";
const MENU_FOLLOW_ALL: &str = "follow_all";
const MENU_STATUS: &str = "status";
const MENU_PIN_PANEL: &str = "pin_panel";
const MENU_OPEN_SETTINGS: &str = "open_settings";
const MENU_AUTOSTART: &str = "autostart";
const MENU_TALK: &str = "talk";
/// webview が購読する、入力欄を開くイベント(Phase 10)。
pub const COMPOSER_EVENT: &str = "secretary://composer";
/// webview が購読する、パネル固定の切り替えイベント。
pub const PANEL_PIN_EVENT: &str = "secretary://panel-pin";
const MENU_DEMO_PLAY: &str = "demo_play";
/// デバッグメニューの状態項目は `demo_state_<name>`。
const MENU_DEMO_STATE_PREFIX: &str = "demo_state_";

/// ウィンドウ位置の保存ファイル名(app_config_dir 直下)。
const STATE_FILENAME: &str = "window-state.json";
/// 初回配置のときの画面端からの余白(論理ピクセル)。
const EDGE_MARGIN: f64 = 24.0;
/// ウィンドウの基準サイズ(scale = 1.0)。tauri.conf.json と styles.css の #stage と合わせる。
const BASE_WIDTH: f64 = 280.0;
const BASE_HEIGHT: f64 = 444.0;
/// キャラクター(#figure)の大きさ(scale = 1.0)。index.html / styles.css と合わせる。
const FIGURE_WIDTH: f64 = 240.0;
const FIGURE_HEIGHT: f64 = 320.0;
/// キャラクターの矩形の周囲で、まだ「触れている」とみなす余白(論理ピクセル)。
const HOVER_MARGIN: f64 = 16.0;

/// トレイは最後のハンドルが破棄されると消える参照カウント方式なので、アプリ状態で保持する。
/// `status` は追跡中セッションと状態を表示する無効化された項目。
struct TrayHandle<R: Runtime> {
    #[allow(dead_code)]
    tray: TrayIcon<R>,
    status: MenuItem<R>,
}

/// ユーザー操作に関わる切り替え状態。
#[derive(Default)]
pub struct Interaction {
    /// トレイの「クリック透過(常時)」。true なら常にすり抜ける
    manual_click_through: AtomicBool,
    /// 右クリックメニューの「今のタスクを表示」。true ならパネルを出しっぱなしにする
    panel_pinned: AtomicBool,
    /// webview が入力欄や許可ボタンを出している間 true。ウィンドウ全体を当たり判定にする
    extended: AtomicBool,
}

fn status_label(state: AssistantState) -> &'static str {
    match state {
        AssistantState::Idle => "待機中",
        AssistantState::Thinking => "考え中",
        AssistantState::Working => "作業中",
        AssistantState::Waiting => "許可待ち",
        AssistantState::Success => "完了",
        AssistantState::Error => "エラー",
    }
}

/// トレイの状態行を更新する。bridge::publish から呼ばれる。
pub fn update_tray_status<R: Runtime>(app: &AppHandle<R>, snapshot: &SecretarySnapshot) {
    let Some(handle) = app.try_state::<TrayHandle<R>>() else {
        return;
    };
    let text = match &snapshot.session_label {
        Some(label) => format!("{} · {}", label, status_label(snapshot.status)),
        None => "追跡中のセッションなし".to_string(),
    };
    if let Err(e) = handle.status.set_text(text) {
        eprintln!("tray status update failed: {e}");
    }
}

/// カーソル(論理座標)がキャラクターの矩形(余白込み)に入っているか。
/// ウィンドウは基準サイズの scale 倍なので、キャラクターの矩形も同じ倍率で見る。
fn cursor_over_figure(
    cursor: LogicalPosition<f64>,
    window_pos: LogicalPosition<f64>,
    window_size: LogicalSize<f64>,
) -> bool {
    let scale = (window_size.width / BASE_WIDTH).max(0.1);
    let (fw, fh, margin) = (
        FIGURE_WIDTH * scale,
        FIGURE_HEIGHT * scale,
        HOVER_MARGIN * scale,
    );
    let fx = window_pos.x + (window_size.width - fw) / 2.0;
    let fy = window_pos.y + window_size.height - fh;
    cursor.x >= fx - margin
        && cursor.x < fx + fw + margin
        && cursor.y >= fy - margin
        && cursor.y < fy + fh + margin
}

/// キャラクターウィンドウを基準サイズの `scale` 倍にする。webview 側は幅から倍率を読む。
pub fn apply_scale<R: Runtime>(app: &AppHandle<R>, scale: f64) {
    let Some(window) = character_window(app) else {
        return;
    };
    let scale = scale.clamp(config::MIN_SCALE, config::MAX_SCALE);
    if let Err(e) = window.set_size(LogicalSize::new(BASE_WIDTH * scale, BASE_HEIGHT * scale)) {
        eprintln!("set_size failed: {e}");
    }
}

/// カーソルがウィンドウの矩形に入っているか。入力欄やボタンを出している間はこちらで判定する。
fn cursor_in_window(
    cursor: LogicalPosition<f64>,
    window_pos: LogicalPosition<f64>,
    window_size: LogicalSize<f64>,
) -> bool {
    cursor.x >= window_pos.x
        && cursor.x < window_pos.x + window_size.width
        && cursor.y >= window_pos.y
        && cursor.y < window_pos.y + window_size.height
}

/// カーソルがキャラクターの外にある間だけクリックをすり抜けさせる。
/// すり抜け中はウィンドウにマウスイベントが届かないので、Rust 側で定期的に位置を見る。
async fn cursor_watch(app: AppHandle, ui: Arc<Interaction>) {
    let mut interval = tokio::time::interval(Duration::from_millis(60));
    let mut last: Option<bool> = None;
    loop {
        interval.tick().await;
        let Some(window) = character_window(&app) else {
            continue;
        };
        let desired = if ui.manual_click_through.load(Ordering::Relaxed) {
            true
        } else {
            let over = (|| -> tauri::Result<bool> {
                let primary_scale = window
                    .primary_monitor()?
                    .map(|m| m.scale_factor())
                    .unwrap_or(1.0);
                let cursor = window.cursor_position()?.to_logical::<f64>(primary_scale);
                let pos = window_logical_position(&window)?;
                let size = window_logical_size(&window)?;
                Ok(if ui.extended.load(Ordering::Relaxed) {
                    cursor_in_window(cursor, pos, size)
                } else {
                    cursor_over_figure(cursor, pos, size)
                })
            })()
            .unwrap_or(true);
            !over
        };
        if last != Some(desired) {
            match window.set_ignore_cursor_events(desired) {
                Ok(()) => {
                    eprintln!("[hover] ignore_cursor_events={desired}");
                    last = Some(desired);
                }
                Err(e) => eprintln!("set_ignore_cursor_events failed: {e}"),
            }
        }
    }
}

/// 入力欄を開く。メニューから呼ぶので、キー入力を受けられるようウィンドウにフォーカスも移す。
fn open_composer<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = character_window(app) {
        if let Err(e) = window.set_focus() {
            eprintln!("set_focus failed: {e}");
        }
    }
    if let Err(e) = app.emit_to(CHARACTER_WINDOW, COMPOSER_EVENT, true) {
        eprintln!("emit composer failed: {e}");
    }
}

/// webview が入力欄や許可ボタンを出し入れしたときに呼ぶ。
#[tauri::command]
fn set_interactive(ui: tauri::State<'_, Arc<Interaction>>, extended: bool) {
    ui.extended.store(extended, Ordering::Relaxed);
}

fn set_panel_pinned<R: Runtime>(app: &AppHandle<R>, ui: &Interaction, pinned: bool) {
    ui.panel_pinned.store(pinned, Ordering::Relaxed);
    if let Err(e) = app.emit_to(CHARACTER_WINDOW, PANEL_PIN_EVENT, pinned) {
        eprintln!("emit panel pin failed: {e}");
    }
}

/// キャラクター上の右クリックで出すメニュー。webview から呼ばれる。
#[tauri::command]
fn show_context_menu(app: AppHandle, window: tauri::Window) -> Result<(), String> {
    let pinned = app
        .try_state::<Arc<Interaction>>()
        .map(|ui| ui.panel_pinned.load(Ordering::Relaxed))
        .unwrap_or(false);
    let build = || -> tauri::Result<Menu<tauri::Wry>> {
        Menu::with_items(
            &app,
            &[
                &MenuItem::with_id(&app, MENU_TALK, "話しかける…", true, None::<&str>)?,
                &PredefinedMenuItem::separator(&app)?,
                &CheckMenuItem::with_id(
                    &app,
                    MENU_PIN_PANEL,
                    "今のタスクを表示",
                    true,
                    pinned,
                    None::<&str>,
                )?,
                &PredefinedMenuItem::separator(&app)?,
                &MenuItem::with_id(&app, MENU_OPEN_SETTINGS, "設定…", true, None::<&str>)?,
            ],
        )
    };
    let menu = build().map_err(|e| e.to_string())?;
    // 項目の処理はトレイの on_menu_event が担う(メニューイベントはアプリ全体に届く)
    menu.popup(window).map_err(|e| e.to_string())
}

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
    let status = MenuItem::with_id(
        app,
        MENU_STATUS,
        "追跡中のセッションなし",
        false,
        None::<&str>,
    )?;
    let click_through = CheckMenuItem::with_id(
        app,
        MENU_CLICK_THROUGH,
        "クリック透過(常時)",
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
    let follow_all_initial = app
        .try_state::<Arc<Core>>()
        .map(|c| c.follow() == FollowPolicy::All)
        .unwrap_or(false);
    let follow_all = CheckMenuItem::with_id(
        app,
        MENU_FOLLOW_ALL,
        "すべてのセッションを追跡",
        true,
        follow_all_initial,
        None::<&str>,
    )?;
    let open_settings = MenuItem::with_id(app, MENU_OPEN_SETTINGS, "設定…", true, None::<&str>)?;
    // 開発版の実行ファイルはビルド版の場所に無いので、自動起動はビルド版でだけ切り替えられる
    let autostart_available = !cfg!(debug_assertions);
    let autostart_initial = {
        use tauri_plugin_autostart::ManagerExt;
        autostart_available && app.autolaunch().is_enabled().unwrap_or(false)
    };
    let autostart = CheckMenuItem::with_id(
        app,
        MENU_AUTOSTART,
        if autostart_available {
            "ログイン時に起動"
        } else {
            "ログイン時に起動(ビルド版のみ)"
        },
        autostart_available,
        autostart_initial,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "終了", true, None::<&str>)?;
    let talk = MenuItem::with_id(app, MENU_TALK, "話しかける…", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &talk,
            &PredefinedMenuItem::separator(app)?,
            &click_through,
            &always_on_top,
            &follow_all,
            &autostart,
            &open_settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    if cfg!(debug_assertions) {
        // 開発中だけ: 状態を手で切り替えて見た目を確認する
        let mut items: Vec<MenuItem<R>> = Vec::new();
        items.push(MenuItem::with_id(
            app,
            MENU_DEMO_PLAY,
            "デモを一巡再生",
            true,
            None::<&str>,
        )?);
        for state in bridge::ALL_STATES {
            let name = bridge::state_name(state);
            items.push(MenuItem::with_id(
                app,
                format!("{MENU_DEMO_STATE_PREFIX}{name}"),
                format!("状態: {name}"),
                true,
                None::<&str>,
            )?);
        }
        let refs: Vec<&dyn tauri::menu::IsMenuItem<R>> = items
            .iter()
            .map(|i| i as &dyn tauri::menu::IsMenuItem<R>)
            .collect();
        let debug = Submenu::with_items(app, "デバッグ", true, &refs)?;
        menu.insert(&debug, 8)?;
    }

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
                // 実際の切り替えは cursor_watch が行う
                let on = click_through.is_checked().unwrap_or(false);
                if let Some(ui) = app.try_state::<Arc<Interaction>>() {
                    ui.manual_click_through.store(on, Ordering::Relaxed);
                }
            }
            MENU_PIN_PANEL => {
                if let Some(ui) = app.try_state::<Arc<Interaction>>() {
                    let pinned = !ui.panel_pinned.load(Ordering::Relaxed);
                    set_panel_pinned(app, &ui, pinned);
                }
            }
            MENU_OPEN_SETTINGS => settings::open_settings_window(app),
            MENU_AUTOSTART => {
                use tauri_plugin_autostart::ManagerExt;
                let on = autostart.is_checked().unwrap_or(false);
                let launcher = app.autolaunch();
                let result = if on {
                    launcher.enable()
                } else {
                    launcher.disable()
                };
                match result {
                    Ok(()) => eprintln!("[autostart] {}", if on { "enabled" } else { "disabled" }),
                    Err(e) => {
                        eprintln!("[autostart] failed: {e}");
                        let _ = autostart.set_checked(!on);
                    }
                }
            }
            MENU_TALK => open_composer(app),
            MENU_ALWAYS_ON_TOP => {
                let on = always_on_top.is_checked().unwrap_or(true);
                if let Some(window) = character_window(app) {
                    if let Err(e) = window.set_always_on_top(on) {
                        eprintln!("set_always_on_top failed: {e}");
                    }
                }
            }
            MENU_FOLLOW_ALL => {
                let all = follow_all.is_checked().unwrap_or(false);
                if let Some(core) = app.try_state::<Arc<Core>>() {
                    core.set_follow(if all {
                        FollowPolicy::All
                    } else {
                        FollowPolicy::Channel
                    });
                    server::refresh(app, &core);
                }
            }
            MENU_QUIT => {
                if let Err(e) = save_position(app) {
                    eprintln!("save_position failed: {e}");
                }
                app.exit(0);
            }
            MENU_DEMO_PLAY => bridge::play_demo_once(app.clone(), Duration::from_secs(3)),
            id => {
                if let Some(state) = id
                    .strip_prefix(MENU_DEMO_STATE_PREFIX)
                    .and_then(bridge::state_from_name)
                {
                    bridge::publish(app, bridge::demo_snapshot(state));
                }
            }
        })
        .build(app)?;
    app.manage(TrayHandle { tray, status });

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
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_notification::init())
        .manage(SnapshotBridge::new())
        // 設定ウィンドウは閉じても隠すだけにして、次に開くときに作り直さない
        .on_window_event(|window, event| {
            if window.label() == settings::SETTINGS_WINDOW {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            // Dock にアイコンを出さない(macOS)。skipTaskbar は Windows / Linux 専用。
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // 設定と認証トークンを読み、Tracker を包む Core を用意する
            let config = config::load_or_create(app.handle());
            let token = config::load_or_create_token(app.handle())?;
            let persona = persona::Persona::new(persona::load_or_create(app.handle()));
            let core = Arc::new(Core::new(config.clone(), persona, token));
            app.manage(core.clone());
            let ui = Arc::new(Interaction::default());
            app.manage(ui.clone());

            build_tray(app.handle())?;
            restore_and_show(app.handle())?;
            apply_scale(app.handle(), config.scale);

            // hook 受信サーバーと、一時状態の期限切れを反映する定期処理
            tauri::async_runtime::spawn(server::serve(
                app.handle().clone(),
                core.clone(),
                config.port,
            ));
            // 秘書 → Claude Code の channel(Phase 10)。secretary-channel 子プロセスがここへ繋ぐ
            let socket = app.path().app_config_dir()?.join(CHANNEL_SOCKET_FILENAME);
            tauri::async_runtime::spawn(channel::serve(app.handle().clone(), core.clone(), socket));
            tauri::async_runtime::spawn(server::ticker(app.handle().clone(), core));
            tauri::async_runtime::spawn(cursor_watch(app.handle().clone(), ui));
            bridge::start_demo_loop_if_requested(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            frontend_log,
            bridge::get_snapshot,
            show_context_menu,
            set_interactive,
            channel::send_prompt,
            channel::respond_permission,
            settings::settings_info,
            settings::save_config,
            settings::set_autostart,
            settings::open_path,
            settings::reload_persona
        ])
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

#[cfg(test)]
mod tests {
    use super::*;

    fn win() -> (LogicalPosition<f64>, LogicalSize<f64>) {
        (
            LogicalPosition::new(100.0, 200.0),
            LogicalSize::new(280.0, 420.0),
        )
    }

    #[test]
    fn cursor_inside_figure_is_over() {
        let (p, z) = win();
        // figure は x 120..360, y 300..620
        assert!(cursor_over_figure(LogicalPosition::new(200.0, 500.0), p, z));
        assert!(cursor_over_figure(LogicalPosition::new(121.0, 301.0), p, z));
    }

    #[test]
    fn cursor_in_bubble_area_or_outside_is_not_over() {
        let (p, z) = win();
        // 吹き出しの余白(上部 100px)は対象外
        assert!(!cursor_over_figure(
            LogicalPosition::new(200.0, 230.0),
            p,
            z
        ));
        // ウィンドウの外
        assert!(!cursor_over_figure(LogicalPosition::new(50.0, 500.0), p, z));
        assert!(!cursor_over_figure(
            LogicalPosition::new(200.0, 700.0),
            p,
            z
        ));
    }

    #[test]
    fn figure_rect_scales_with_the_window() {
        // 2 倍: figure は x 140..620, y 400..1040、余白 32
        let (p, z) = (
            LogicalPosition::new(100.0, 200.0),
            LogicalSize::new(560.0, 840.0),
        );
        assert!(cursor_over_figure(LogicalPosition::new(150.0, 500.0), p, z));
        assert!(cursor_over_figure(LogicalPosition::new(110.0, 500.0), p, z));
        assert!(!cursor_over_figure(
            LogicalPosition::new(100.0, 500.0),
            p,
            z
        ));
        assert!(!cursor_over_figure(
            LogicalPosition::new(300.0, 360.0),
            p,
            z
        ));
    }

    #[test]
    fn hover_margin_keeps_drag_alive_near_the_edge() {
        let (p, z) = win();
        assert!(cursor_over_figure(
            LogicalPosition::new(120.0 - HOVER_MARGIN + 1.0, 500.0),
            p,
            z
        ));
        assert!(!cursor_over_figure(
            LogicalPosition::new(120.0 - HOVER_MARGIN - 1.0, 500.0),
            p,
            z
        ));
    }
}
