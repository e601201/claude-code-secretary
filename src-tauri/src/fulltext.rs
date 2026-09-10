//! 全文ビュー(Issue #1)。吹き出しが出しきれなかった応答本文を読むための独立ウィンドウ。
//!
//! 吹き出しは通知に徹する(ADR-0001)ので、全文はここで読ませる。控えは `Core` が持つ
//! (ADR-0002)ので、このモジュールはウィンドウの開け閉めと受け渡しだけを扱う。
//!
//! 開いている間は中身を差し替えない。読んでいる最中に内容が消えるのは、この Issue の
//! 出発点そのものだから。最新を見たいときは開き直す(そのたびに `FULLTEXT_EVENT` が飛ぶ)。

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::server::{Core, FullSpeech};

/// `tauri.conf.json` の windows[].label と一致させる。
pub const FULLTEXT_WINDOW: &str = "fulltext";

/// webview が購読する、控えを読み直すイベント。ウィンドウを出すたびに飛ばす。
pub const FULLTEXT_EVENT: &str = "secretary://fulltext";

/// 全文ビューを出す(閉じても隠すだけなので、何度でも出せる)。
pub fn open_fulltext_window<R: Runtime>(app: &AppHandle<R>) {
    if !crate::show_window(app, FULLTEXT_WINDOW) {
        return;
    }
    // 出すたびに読み直させる。これで「開いた時点の 1 件に固定」が成立する
    if let Err(e) = app.emit_to(FULLTEXT_WINDOW, FULLTEXT_EVENT, ()) {
        eprintln!("[fulltext] emit failed: {e}");
    }
}

/// 控えがあるか。メニュー項目の有効 / 無効に使う。
pub fn has_speech<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<Arc<Core>>()
        .is_some_and(|core| core.has_full_speech())
}

/// 全文ビューの webview が読みに来る控え。まだ一度も応答が無ければ `None`。
#[tauri::command]
pub fn full_speech(core: tauri::State<'_, Arc<Core>>) -> Option<FullSpeech> {
    core.full_speech()
}
