//! Phase 0 で記録した Discord 経由の実イベント(個人情報を除去済み)を再生し、
//! 各イベント後の表示状態が仕様どおりになることを確かめる。

use std::time::{Duration, Instant};

use secretary_core::{AssistantState, SpeechKind, Tracker, TrackerConfig};

const FIXTURE: &str = include_str!("fixtures/discord_turns.jsonl");

use AssistantState::*;

#[test]
fn replay_three_discord_turns() {
    let mut tracker = Tracker::new(TrackerConfig::default());
    let mut now = Instant::now();

    // 既定の追跡方針は Discord 由来のセッションだけ。何も届く前は idle。
    assert_eq!(tracker.snapshot(now).status, Idle);

    let expected = [
        // ターン1: README を読んで要約
        ("UserPromptSubmit", Thinking),
        ("PreToolUse", Thinking), // Bash: ls / cat / head は読み取り
        ("PostToolUse", Thinking),
        ("PreToolUse", Thinking), // ToolSearch は補助
        ("PostToolUse", Thinking),
        ("PreToolUse", Working), // Discord への返信
        ("PostToolUse", Thinking),
        ("Stop", Success),
        // ターン2: /tmp に空ファイルを作る(auto mode で自動承認)
        ("UserPromptSubmit", Thinking),
        ("PreToolUse", Working), // Bash: touch
        ("PostToolUse", Thinking),
        ("PreToolUse", Working),
        ("PostToolUse", Thinking),
        ("Stop", Success),
        // ターン3: 同じ依頼(Deny のつもりだったが自動承認)
        ("UserPromptSubmit", Thinking),
        ("PreToolUse", Working),
        ("PostToolUse", Thinking),
        ("PreToolUse", Working),
        ("PostToolUse", Thinking),
        ("Stop", Success),
        // 約 60 秒後に idle_prompt 通知、その後セッション終了
        ("Notification", Idle),
        ("SessionEnd", Idle),
    ];

    let lines: Vec<&str> = FIXTURE.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), expected.len(), "fixture length changed");

    for (i, (line, (name, want))) in lines.iter().zip(expected.iter()).enumerate() {
        now += if *name == "Notification" {
            Duration::from_secs(60)
        } else {
            Duration::from_millis(100)
        };
        let event = tracker.apply_json(line, now).expect("valid hook json");
        let event_name = format!("{event:?}");
        assert!(
            event_name.starts_with(name),
            "event #{i}: expected {name}, got {event_name}"
        );
        let snap = tracker.snapshot(now);
        assert_eq!(snap.status, *want, "event #{i} ({name}) status");

        match i {
            0 => assert_eq!(
                snap.task_summary.as_deref(),
                Some("このプロジェクトのREADMEを読んで、内容を3行で要約して")
            ),
            5 => {
                assert_eq!(snap.message_kind, Some(SpeechKind::Reply));
                assert!(snap
                    .message
                    .as_deref()
                    .unwrap()
                    .starts_with("README の要約"));
                assert!(snap
                    .current_tool
                    .as_deref()
                    .unwrap()
                    .starts_with("mcp__plugin_discord_discord__reply"));
            }
            7 => {
                // Stop 後も Discord への返信本文が吹き出しに残る(last_assistant_message は使わない)
                assert_eq!(snap.message_kind, Some(SpeechKind::Reply));
                assert_eq!(snap.session_label.as_deref(), Some("tauri-app"));
                assert_eq!(snap.tracked_sessions, 1);
            }
            9 => assert!(snap
                .current_tool
                .as_deref()
                .unwrap()
                .starts_with("Bash: touch /tmp/secretary-spike-allow.txt")),
            13 => assert_eq!(snap.message.as_deref(), Some("作りました")),
            21 => assert_eq!(snap.tracked_sessions, 0),
            _ => {}
        }
    }
}

#[test]
fn success_hold_expires_to_idle_between_turns() {
    let mut tracker = Tracker::new(TrackerConfig::default());
    let mut now = Instant::now();
    for line in FIXTURE.lines().take(8) {
        now += Duration::from_millis(100);
        tracker.apply_json(line, now).unwrap();
    }
    assert_eq!(tracker.snapshot(now).status, Success);
    assert_eq!(tracker.snapshot(now + Duration::from_secs(3)).status, Idle);
}
