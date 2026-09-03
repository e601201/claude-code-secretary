//! 通知(Phase 11)。許可待ちに入ったときに OS の通知を出す。
//!
//! ビルド版は `tauri-plugin-notification` を使う。開発版(`tauri dev`)の実行ファイルは .app に
//! 入っていないので macOS の通知センターに登録できず、代わりに `osascript` で出す。

use secretary_core::{AssistantState, SecretarySnapshot};
use tauri::{AppHandle, Runtime};

const TITLE: &str = "秘書";

/// 前回の状態から `next` へ変わったとき、通知すべきか。許可待ちに「入った」瞬間だけ。
pub fn should_notify(prev: Option<AssistantState>, next: AssistantState) -> bool {
    next == AssistantState::Waiting && prev != Some(AssistantState::Waiting)
}

/// 通知の本文。吹き出しの文言(人格で飾ったもの)があればそれ、無ければ定型。
pub fn body_for(snapshot: &SecretarySnapshot) -> String {
    let core = match snapshot.message.as_deref().filter(|m| !m.trim().is_empty()) {
        Some(m) => m.trim().to_string(),
        None => match &snapshot.pending_permission {
            Some(tool) => format!("{tool} の実行許可を待っています"),
            None => "許可を待っています".to_string(),
        },
    };
    let head: String = core
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(120)
        .collect();
    match &snapshot.session_label {
        Some(label) => format!("[{label}] {head}"),
        None => head,
    }
}

/// スナップショットの変化を受け取り、必要なら通知する。`server::refresh` から呼ぶ。
pub fn on_transition<R: Runtime>(
    app: &AppHandle<R>,
    prev: Option<AssistantState>,
    next: &SecretarySnapshot,
) {
    if !should_notify(prev, next.status) {
        return;
    }
    let body = body_for(next);
    eprintln!("[notify] {body}");
    send(app, &body);
}

#[cfg(debug_assertions)]
fn send<R: Runtime>(_app: &AppHandle<R>, body: &str) {
    // 開発版: osascript。表示上の送り主は「Script Editor」になる
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        applescript_escape(body),
        TITLE
    );
    if let Err(e) = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .spawn()
    {
        eprintln!("[notify] osascript failed: {e}");
    }
}

#[cfg(not(debug_assertions))]
fn send<R: Runtime>(app: &AppHandle<R>, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = app.notification().builder().title(TITLE).body(body).show() {
        eprintln!("[notify] failed: {e}");
    }
}

#[cfg(debug_assertions)]
fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(status: AssistantState, message: Option<&str>) -> SecretarySnapshot {
        SecretarySnapshot {
            status,
            message: message.map(str::to_string),
            session_label: Some("life".into()),
            pending_permission: Some("Bash".into()),
            ..SecretarySnapshot::idle()
        }
    }

    #[test]
    fn notifies_only_when_entering_waiting() {
        assert!(should_notify(None, AssistantState::Waiting));
        assert!(should_notify(
            Some(AssistantState::Working),
            AssistantState::Waiting
        ));
        assert!(!should_notify(
            Some(AssistantState::Waiting),
            AssistantState::Waiting
        ));
        assert!(!should_notify(
            Some(AssistantState::Waiting),
            AssistantState::Working
        ));
        assert!(!should_notify(None, AssistantState::Error));
    }

    #[test]
    fn body_prefers_bubble_text_and_labels_the_session() {
        assert_eq!(
            body_for(&snap(
                AssistantState::Waiting,
                Some("Bash を実行してよいか、確認させてください。\n二行目")
            )),
            "[life] Bash を実行してよいか、確認させてください。"
        );
        assert_eq!(
            body_for(&snap(AssistantState::Waiting, None)),
            "[life] Bash の実行許可を待っています"
        );
        let mut no_label = snap(AssistantState::Waiting, Some("  x  "));
        no_label.session_label = None;
        assert_eq!(body_for(&no_label), "x");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn applescript_quotes_are_escaped() {
        assert_eq!(applescript_escape(r#"a "b" \c"#), r#"a \"b\" \\c"#);
    }
}
