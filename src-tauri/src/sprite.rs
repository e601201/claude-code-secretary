//! 立ち絵のシート(`<設定フォルダ>/character.png`)を検める。仕様は `docs/character-sheet.md`。
//!
//! 読むのは PNG のヘッダ(IHDR)だけで、画素には触れない。だから画像ライブラリは要らない。
//! 検証を Rust に置いてあるのは、キャラクターウィンドウと設定ウィンドウが**同じ判定**を
//! 見るため。片方の webview に判定させると、もう片方がシートをもう一度デコードすることになる。

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};
use ts_rs::TS;

use crate::{config, CHARACTER_WINDOW};

/// シートのファイル名(設定フォルダ直下)。
pub const SHEET_FILENAME: &str = "character.png";
/// 行 = 状態。順序は `bridge::ALL_STATES` と `docs/state-machine.md` の状態表に合わせる。
pub const ROWS: u32 = 6;
/// 列 = コマ。
pub const COLS: u32 = 4;
/// webview が購読する、立ち絵が差し替わったことを知らせるイベント。
pub const SHEET_EVENT: &str = "secretary://sheet";

/// PNG のヘッダを読むのに要る長さ(署名 8 + 長さ 4 + 種別 4 + IHDR 13 の先頭 8)。
const HEADER_BYTES: usize = 24;
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// 立ち絵の状態。設定ウィンドウはこれをそのまま出し、キャラクターウィンドウはこれで
/// シートとプレースホルダーのどちらを使うか決める。
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct SheetStatus {
    /// シートの絶対パス。置かれていなくても返す(設定ウィンドウが「ここに置く」と示すため)
    pub path: String,
    /// そのまま立ち絵として使えるか
    pub usable: bool,
    /// 使えない理由。`usable` が true のときは None
    pub reason: Option<String>,
    /// 1 コマの寸法(使えるときだけ)
    pub frame_width: Option<u32>,
    pub frame_height: Option<u32>,
    /// シート全体の寸法(PNG として読めたときだけ)
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// 差し替えを webview のキャッシュに気付かせるための版(更新時刻のミリ秒)
    pub version: f64,
}

impl SheetStatus {
    fn unusable(path: &Path, reason: impl Into<String>) -> Self {
        Self {
            path: path.display().to_string(),
            usable: false,
            reason: Some(reason.into()),
            frame_width: None,
            frame_height: None,
            width: None,
            height: None,
            version: 0.0,
        }
    }
}

/// PNG の先頭から寸法を読む。PNG でなければ None。
pub fn png_size(head: &[u8]) -> Option<(u32, u32)> {
    if head.len() < HEADER_BYTES || head[..8] != PNG_SIGNATURE || &head[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(head[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(head[20..24].try_into().ok()?);
    Some((w, h))
}

/// 寸法が仕様を満たすか。満たせば 1 コマの寸法を返す。
///
/// 必須はコマの境界がずれないことだけ。1 コマの縦横比(推奨 3:4)は強制しない
/// —— 外れていても `contain` が吸収して小さく出るだけで、破綻はしない。
pub fn validate(width: u32, height: u32) -> Result<(u32, u32), String> {
    if width == 0 || height == 0 {
        return Err("寸法が 0 です".to_string());
    }
    if !width.is_multiple_of(COLS) {
        return Err(format!(
            "幅 {width}px が {COLS}(コマ数)で割り切れません。{COLS} の倍数にしてください"
        ));
    }
    if !height.is_multiple_of(ROWS) {
        return Err(format!(
            "高さ {height}px が {ROWS}(状態の数)で割り切れません。{ROWS} の倍数にしてください"
        ));
    }
    Ok((width / COLS, height / ROWS))
}

/// パスを検める。ファイルが無い・PNG でない・寸法が合わないのどれでも、
/// 理由を持った `SheetStatus` を返す(呼び出し側で分岐させない)。
pub fn inspect(path: &Path) -> SheetStatus {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return SheetStatus::unusable(path, "まだ置かれていません")
        }
        Err(e) => return SheetStatus::unusable(path, format!("開けません: {e}")),
    };

    let mut head = [0u8; HEADER_BYTES];
    if file.read_exact(&mut head).is_err() {
        return SheetStatus::unusable(path, "PNG として読めません");
    }
    let Some((width, height)) = png_size(&head) else {
        return SheetStatus::unusable(path, "PNG として読めません");
    };

    let version = file
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0);

    match validate(width, height) {
        Ok((frame_width, frame_height)) => SheetStatus {
            path: path.display().to_string(),
            usable: true,
            reason: None,
            frame_width: Some(frame_width),
            frame_height: Some(frame_height),
            width: Some(width),
            height: Some(height),
            version,
        },
        Err(reason) => SheetStatus {
            width: Some(width),
            height: Some(height),
            ..SheetStatus::unusable(path, reason)
        },
    }
}

/// 設定フォルダ直下のシートのパス。設定フォルダが取れないときは None。
pub fn sheet_path<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    config::config_dir(app).ok().map(|d| d.join(SHEET_FILENAME))
}

/// 今のシートの状態。設定フォルダが取れないときも理由付きで返す。
pub fn status<R: Runtime>(app: &AppHandle<R>) -> SheetStatus {
    match sheet_path(app) {
        Some(path) => inspect(&path),
        None => SheetStatus::unusable(Path::new(SHEET_FILENAME), "設定フォルダが見つかりません"),
    }
}

/// キャラクターウィンドウに、立ち絵を読み直させる。
pub fn publish<R: Runtime>(app: &AppHandle<R>, status: &SheetStatus) {
    if let Err(e) = app.emit_to(CHARACTER_WINDOW, SHEET_EVENT, status) {
        eprintln!("emit sheet failed: {e}");
    }
}

/// キャラクターウィンドウが起動直後に取りに来る。
#[tauri::command]
pub fn sheet_status(app: AppHandle) -> SheetStatus {
    status(&app)
}

/// 設定ウィンドウの「立ち絵を再読み込み」。検め直して、キャラクター側へ流す。
#[tauri::command]
pub fn reload_sheet(app: AppHandle) -> SheetStatus {
    let status = status(&app);
    match &status.reason {
        Some(reason) => eprintln!("[sheet] {} ({reason})", status.path),
        None => eprintln!(
            "[sheet] {} ({}x{}, 1 コマ {}x{})",
            status.path,
            status.width.unwrap_or(0),
            status.height.unwrap_or(0),
            status.frame_width.unwrap_or(0),
            status.frame_height.unwrap_or(0)
        ),
    }
    publish(&app, &status);
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 幅 w、高さ h の PNG のヘッダだけを組み立てる。
    fn header(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::from(PNG_SIGNATURE);
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v
    }

    #[test]
    fn reads_size_from_a_png_header() {
        assert_eq!(png_size(&header(1920, 3840)), Some((1920, 3840)));
    }

    #[test]
    fn rejects_anything_that_is_not_a_png() {
        assert_eq!(png_size(b"not a png at all......."), None);
        assert_eq!(png_size(&[]), None);
        // 署名は合っていても IHDR で始まらないもの
        let mut broken = header(8, 8);
        broken[12..16].copy_from_slice(b"IDAT");
        assert_eq!(png_size(&broken), None);
    }

    #[test]
    fn accepts_a_sheet_that_divides_into_frames() {
        assert_eq!(validate(1920, 3840), Ok((480, 640)));
        // 推奨の 3:4 から外れていても、割り切れれば通す
        assert_eq!(validate(400, 600), Ok((100, 100)));
    }

    #[test]
    fn rejects_a_sheet_whose_frames_would_not_line_up() {
        assert!(validate(1921, 3840).unwrap_err().contains("幅"));
        assert!(validate(1920, 3841).unwrap_err().contains("高さ"));
        assert!(validate(0, 3840).is_err());
    }

    #[test]
    fn a_missing_sheet_says_so_instead_of_failing() {
        let status = inspect(Path::new("/nonexistent/character.png"));
        assert!(!status.usable);
        assert_eq!(status.reason.as_deref(), Some("まだ置かれていません"));
        assert!(status.frame_width.is_none());
    }

    #[test]
    fn an_unusable_sheet_still_reports_the_size_it_had() {
        let dir = std::env::temp_dir().join("secretary-sprite-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("odd.png");
        std::fs::write(&path, header(1921, 3840)).unwrap();

        let status = inspect(&path);
        assert!(!status.usable);
        assert_eq!(status.width, Some(1921));
        assert!(status.reason.unwrap().contains("割り切れません"));

        std::fs::remove_file(&path).ok();
    }
}
