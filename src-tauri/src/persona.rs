//! 秘書の人格。出来事ごとの定型文を辞書(`persona.toml`)から選び、スナップショットを飾る。
//!
//! 優先順位は「Claude の実際の言葉 > 定型文」。Discord への返信本文や最終応答がある間は
//! 触らず、文言が無いときだけ埋める。許可待ちの文言は言い換え、失敗の文言には一言を添える。

use std::{
    collections::HashMap,
    fs,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use secretary_core::{AssistantState, HookEvent, SecretarySnapshot, SpeechKind};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};

pub const PERSONA_FILENAME: &str = "persona.toml";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PersonaConfig {
    pub phrases: Phrases,
    pub timing: Timing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Phrases {
    /// 指示を受け取った直後
    pub turn_start: Vec<String>,
    /// 作業が長引いているとき(timing.long_work_after_secs 経過後に一度だけ)
    pub long_work: Vec<String>,
    /// 失敗なくターンが終わり、Claude の言葉が無いとき
    pub success: Vec<String>,
    /// ツール失敗や応答失敗の文言の前に添える一言
    pub error: Vec<String>,
    /// 許可待ちの言い換え。`{tool}` はツール名に置き換わる
    pub waiting: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Timing {
    pub long_work_after_secs: f64,
}

impl Default for Phrases {
    fn default() -> Self {
        let v = |items: &[&str]| items.iter().map(|s| s.to_string()).collect();
        Self {
            turn_start: v(&[
                "はい、お任せください！",
                "承知しました。取りかかりますね。",
                "了解です。見てみます。",
            ]),
            long_work: v(&[
                "少々お待ちください……",
                "もう少しかかりそうです。",
                "いま集中して作業しています。",
            ]),
            success: v(&["完了しました！", "できました。ご確認ください。"]),
            error: v(&[
                "……すみません、問題が見つかりました。",
                "うまくいきませんでした。確認します。",
            ]),
            waiting: v(&[
                "「{tool}」の実行に確認が必要です。Discordで承認をお願いします。",
                "{tool} を実行してよいか、確認させてください。",
            ]),
        }
    }
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            long_work_after_secs: 20.0,
        }
    }
}

impl PersonaConfig {
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// 初回に書き出す、コメント付きの辞書。
    pub fn template() -> String {
        let d = Self::default();
        let list = |items: &[String]| {
            items
                .iter()
                .map(|s| format!("  \"{}\",", s.replace('"', "\\\"")))
                .collect::<Vec<_>>()
                .join("\n")
        };
        format!(
            "# 秘書の口調。各項目は候補の配列で、直前と同じ文言は避けて選ぶ。\n\
             # 変更後はアプリを再起動する。\n\
             \n\
             [phrases]\n\
             # 指示を受け取った直後\n\
             turn_start = [\n{turn_start}\n]\n\
             \n\
             # 作業が長引いているとき(timing.long_work_after_secs 経過後に一度だけ)\n\
             long_work = [\n{long_work}\n]\n\
             \n\
             # 失敗なくターンが終わり、Claude の言葉が無いとき\n\
             success = [\n{success}\n]\n\
             \n\
             # ツール失敗や応答失敗の文言の前に添える一言\n\
             error = [\n{error}\n]\n\
             \n\
             # 許可待ちの言い換え。{{tool}} はツール名に置き換わる\n\
             waiting = [\n{waiting}\n]\n\
             \n\
             [timing]\n\
             long_work_after_secs = {long}\n",
            turn_start = list(&d.phrases.turn_start),
            long_work = list(&d.phrases.long_work),
            success = list(&d.phrases.success),
            error = list(&d.phrases.error),
            waiting = list(&d.phrases.waiting),
            long = d.timing.long_work_after_secs,
        )
    }
}

/// 辞書を読む。無ければテンプレートを書き出して既定値を返す。壊れていれば既定値で続行する。
pub fn load_or_create<R: Runtime>(app: &AppHandle<R>) -> PersonaConfig {
    let Ok(dir) = crate::config::config_dir(app) else {
        return PersonaConfig::default();
    };
    let path = dir.join(PERSONA_FILENAME);
    match fs::read_to_string(&path) {
        Ok(text) => match PersonaConfig::parse(&text) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!(
                    "[persona] {} is invalid ({e}); using defaults",
                    path.display()
                );
                PersonaConfig::default()
            }
        },
        Err(_) => {
            match fs::create_dir_all(&dir).and_then(|_| fs::write(&path, PersonaConfig::template()))
            {
                Ok(()) => eprintln!("[persona] wrote default phrases to {}", path.display()),
                Err(e) => eprintln!("[persona] could not write {}: {e}", path.display()),
            }
            PersonaConfig::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Trigger {
    TurnStart,
    LongWork,
    Success,
    Error,
    Waiting,
}

/// 依存を増やさないための小さな乱数(xorshift64)。品質は問わない。
struct XorShift(u64);

impl XorShift {
    fn seeded() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e37_79b9_7f4a_7c15);
        Self(nanos | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// 候補から「直前と同じでないもの」を選ぶ。
struct Chooser {
    cfg: PersonaConfig,
    last_pick: HashMap<Trigger, usize>,
    rng: XorShift,
}

impl Chooser {
    fn pick(&mut self, trigger: Trigger) -> Option<String> {
        let list = match trigger {
            Trigger::TurnStart => &self.cfg.phrases.turn_start,
            Trigger::LongWork => &self.cfg.phrases.long_work,
            Trigger::Success => &self.cfg.phrases.success,
            Trigger::Error => &self.cfg.phrases.error,
            Trigger::Waiting => &self.cfg.phrases.waiting,
        };
        if list.is_empty() {
            return None;
        }
        let mut index = (self.rng.next() % list.len() as u64) as usize;
        if list.len() > 1 {
            if let Some(&last) = self.last_pick.get(&trigger) {
                if index == last {
                    index = (index + 1) % list.len();
                }
            }
        }
        self.last_pick.insert(trigger, index);
        Some(list[index].clone())
    }
}

/// セッションごとの「いま言いたいこと」。
#[derive(Debug, Default, Clone)]
struct Mood {
    /// 文言が無いときに使う定型文
    current: Option<(String, SpeechKind)>,
    /// 失敗文言の前に添える一言
    error_prefix: Option<String>,
    /// 許可待ちの言い換え(ツール名を埋め込み済み)
    waiting_text: Option<String>,
    /// 作業(thinking / working)が続いている開始時刻
    busy_since: Option<Instant>,
    /// このターンで「長引いている」を言ったか
    long_work_said: bool,
}

pub struct Persona {
    chooser: Chooser,
    sessions: HashMap<String, Mood>,
}

impl Persona {
    pub fn new(cfg: PersonaConfig) -> Self {
        Self {
            chooser: Chooser {
                cfg,
                last_pick: HashMap::new(),
                rng: XorShift::seeded(),
            },
            sessions: HashMap::new(),
        }
    }

    fn long_work_after(&self) -> Duration {
        Duration::from_secs_f64(self.chooser.cfg.timing.long_work_after_secs.max(0.0))
    }

    /// hook イベントに反応して、言うことを決めておく。
    pub fn on_event(&mut self, session: &str, event: &HookEvent, now: Instant) {
        match event {
            HookEvent::SessionEnd { .. } => {
                self.sessions.remove(session);
            }
            HookEvent::UserPromptSubmit { .. } => {
                let phrase = self.chooser.pick(Trigger::TurnStart);
                let mood = self.sessions.entry(session.to_string()).or_default();
                *mood = Mood {
                    current: phrase.map(|p| (p, SpeechKind::Assistant)),
                    busy_since: Some(now),
                    ..Mood::default()
                };
            }
            HookEvent::PreToolUse { .. } => {
                let mood = self.sessions.entry(session.to_string()).or_default();
                if mood.busy_since.is_none() {
                    mood.busy_since = Some(now);
                }
            }
            HookEvent::PermissionRequest { tool } => {
                let phrase = self
                    .chooser
                    .pick(Trigger::Waiting)
                    .map(|p| p.replace("{tool}", &tool.name));
                self.sessions
                    .entry(session.to_string())
                    .or_default()
                    .waiting_text = phrase;
            }
            HookEvent::PostToolUseFailure { .. } | HookEvent::StopFailure { .. } => {
                let phrase = self.chooser.pick(Trigger::Error);
                let mood = self.sessions.entry(session.to_string()).or_default();
                mood.error_prefix = phrase;
                if matches!(event, HookEvent::StopFailure { .. }) {
                    mood.busy_since = None;
                }
            }
            HookEvent::Stop {
                last_assistant_message,
                ..
            } => {
                let needs_phrase = last_assistant_message
                    .as_deref()
                    .map(|m| m.trim().is_empty())
                    .unwrap_or(true);
                let phrase = if needs_phrase {
                    self.chooser.pick(Trigger::Success)
                } else {
                    None
                };
                let mood = self.sessions.entry(session.to_string()).or_default();
                mood.busy_since = None;
                if let Some(p) = phrase {
                    mood.current = Some((p, SpeechKind::Assistant));
                }
            }
            _ => {}
        }
    }

    /// スナップショットを飾る。Claude の言葉があるときは触らない。
    pub fn decorate(&mut self, snapshot: &mut SecretarySnapshot, now: Instant) {
        let Some(session) = snapshot.session_id.clone() else {
            return;
        };
        let long_after = self.long_work_after();
        let Some(mood) = self.sessions.get_mut(&session) else {
            return;
        };

        // 長引いている作業への一言(ターンにつき一度)
        let busy = matches!(
            snapshot.status,
            AssistantState::Working | AssistantState::Thinking
        );
        if busy && !mood.long_work_said {
            if let Some(since) = mood.busy_since {
                if now.saturating_duration_since(since) >= long_after {
                    mood.long_work_said = true;
                    if let Some(p) = self.chooser.pick(Trigger::LongWork) {
                        mood.current = Some((p, SpeechKind::Assistant));
                    }
                }
            }
        }

        match (snapshot.message.as_deref(), snapshot.message_kind) {
            (None, _) => {
                if let Some((text, kind)) = &mood.current {
                    snapshot.message = Some(text.clone());
                    snapshot.message_kind = Some(*kind);
                }
            }
            (Some(_), Some(SpeechKind::Permission)) => {
                if let Some(text) = &mood.waiting_text {
                    snapshot.message = Some(text.clone());
                }
            }
            (Some(original), Some(SpeechKind::System)) => {
                if let Some(prefix) = &mood.error_prefix {
                    if !original.starts_with(prefix.as_str()) {
                        snapshot.message = Some(format!("{prefix}\n{original}"));
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secretary_core::ToolRef;
    use serde_json::json;

    fn snapshot(
        status: AssistantState,
        message: Option<&str>,
        kind: Option<SpeechKind>,
    ) -> SecretarySnapshot {
        SecretarySnapshot {
            status,
            message: message.map(str::to_string),
            message_kind: kind,
            current_tool: None,
            task_summary: None,
            pending_permission: None,
            session_id: Some("s".into()),
            session_label: Some("app".into()),
            tracked_sessions: 1,
        }
    }

    fn prompt() -> HookEvent {
        HookEvent::UserPromptSubmit {
            prompt: "x".into(),
            prompt_id: None,
        }
    }

    #[test]
    fn template_parses_to_defaults() {
        assert_eq!(
            PersonaConfig::parse(&PersonaConfig::template()).unwrap(),
            PersonaConfig::default()
        );
    }

    #[test]
    fn chooser_never_repeats_consecutively() {
        let mut p = Persona::new(PersonaConfig::default());
        let mut last: Option<String> = None;
        for _ in 0..50 {
            let pick = p.chooser.pick(Trigger::TurnStart).unwrap();
            assert_ne!(Some(&pick), last.as_ref());
            last = Some(pick);
        }
    }

    #[test]
    fn turn_start_fills_empty_message_but_never_overrides_claude() {
        let mut p = Persona::new(PersonaConfig::default());
        let now = Instant::now();
        p.on_event("s", &prompt(), now);
        let mut snap = snapshot(AssistantState::Thinking, None, None);
        p.decorate(&mut snap, now);
        assert!(PersonaConfig::default()
            .phrases
            .turn_start
            .contains(&snap.message.clone().unwrap()));
        assert_eq!(snap.message_kind, Some(SpeechKind::Assistant));

        let mut snap = snapshot(
            AssistantState::Working,
            Some("直しました！"),
            Some(SpeechKind::Reply),
        );
        p.decorate(&mut snap, now);
        assert_eq!(snap.message.as_deref(), Some("直しました！"));
    }

    #[test]
    fn waiting_is_rephrased_with_tool_name() {
        let mut p = Persona::new(PersonaConfig::default());
        let now = Instant::now();
        p.on_event(
            "s",
            &HookEvent::PermissionRequest {
                tool: ToolRef {
                    name: "Bash".into(),
                    use_id: None,
                    input: json!({}),
                },
            },
            now,
        );
        let mut snap = snapshot(
            AssistantState::Waiting,
            Some("Bash の実行許可を待っています"),
            Some(SpeechKind::Permission),
        );
        p.decorate(&mut snap, now);
        let text = snap.message.unwrap();
        assert!(text.contains("Bash"), "{text}");
        assert!(!text.contains("{tool}"));
        assert_eq!(snap.message_kind, Some(SpeechKind::Permission));
    }

    #[test]
    fn error_gets_a_prefix_once() {
        let mut p = Persona::new(PersonaConfig::default());
        let now = Instant::now();
        p.on_event(
            "s",
            &HookEvent::PostToolUseFailure {
                tool: ToolRef {
                    name: "Bash".into(),
                    use_id: None,
                    input: json!({}),
                },
                error: "Exit code 1".into(),
                is_interrupt: false,
            },
            now,
        );
        let mut snap = snapshot(
            AssistantState::Error,
            Some("Bash が失敗しました: Exit code 1"),
            Some(SpeechKind::System),
        );
        p.decorate(&mut snap, now);
        let first = snap.message.clone().unwrap();
        assert!(
            first.ends_with("\nBash が失敗しました: Exit code 1"),
            "{first}"
        );
        p.decorate(&mut snap, now);
        assert_eq!(
            snap.message.as_deref(),
            Some(first.as_str()),
            "prefix must not stack"
        );
    }

    #[test]
    fn long_work_phrase_after_threshold_once_per_turn() {
        let cfg = PersonaConfig {
            timing: Timing {
                long_work_after_secs: 5.0,
            },
            ..Default::default()
        };
        let mut p = Persona::new(cfg);
        let t0 = Instant::now();
        p.on_event("s", &prompt(), t0);
        let mut snap = snapshot(AssistantState::Working, None, None);
        p.decorate(&mut snap, t0 + Duration::from_secs(2));
        assert!(PersonaConfig::default()
            .phrases
            .turn_start
            .contains(&snap.message.clone().unwrap()));

        let mut snap = snapshot(AssistantState::Working, None, None);
        p.decorate(&mut snap, t0 + Duration::from_secs(6));
        let long = snap.message.clone().unwrap();
        assert!(
            PersonaConfig::default().phrases.long_work.contains(&long),
            "{long}"
        );

        // 同じターンでは二度言わない(文言は据え置き)
        let mut snap = snapshot(AssistantState::Working, None, None);
        p.decorate(&mut snap, t0 + Duration::from_secs(30));
        assert_eq!(snap.message.as_deref(), Some(long.as_str()));

        // 新しいターンでまた言えるようになる
        p.on_event("s", &prompt(), t0 + Duration::from_secs(40));
        let mut snap = snapshot(AssistantState::Working, None, None);
        p.decorate(&mut snap, t0 + Duration::from_secs(46));
        assert!(PersonaConfig::default()
            .phrases
            .long_work
            .contains(&snap.message.clone().unwrap()));
    }

    #[test]
    fn success_phrase_only_when_claude_said_nothing() {
        let mut p = Persona::new(PersonaConfig::default());
        let now = Instant::now();
        p.on_event("s", &prompt(), now);
        p.on_event(
            "s",
            &HookEvent::Stop {
                last_assistant_message: None,
                stop_hook_active: false,
            },
            now,
        );
        let mut snap = snapshot(AssistantState::Success, None, None);
        p.decorate(&mut snap, now);
        assert!(PersonaConfig::default()
            .phrases
            .success
            .contains(&snap.message.clone().unwrap()));

        p.on_event("s", &prompt(), now);
        p.on_event(
            "s",
            &HookEvent::Stop {
                last_assistant_message: Some("終わりました".into()),
                stop_hook_active: false,
            },
            now,
        );
        let mut snap = snapshot(
            AssistantState::Success,
            Some("終わりました"),
            Some(SpeechKind::Assistant),
        );
        p.decorate(&mut snap, now);
        assert_eq!(snap.message.as_deref(), Some("終わりました"));
    }

    #[test]
    fn session_end_forgets_mood() {
        let mut p = Persona::new(PersonaConfig::default());
        let now = Instant::now();
        p.on_event("s", &prompt(), now);
        p.on_event("s", &HookEvent::SessionEnd { reason: None }, now);
        let mut snap = snapshot(AssistantState::Idle, None, None);
        p.decorate(&mut snap, now);
        assert_eq!(snap.message, None);
    }
}
