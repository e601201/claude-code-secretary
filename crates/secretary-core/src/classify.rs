//! ツール呼び出しを「読み取り」「作業」「Discord への返信」「補助」に分類する。
//!
//! Claude は Read ツールではなく Bash の `cat` で読むことがある(Phase 0 で観測)ため、
//! Bash はコマンド文字列を見て判定する。判定は保守的で、迷ったら「作業」に倒す。

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolClass {
    /// 読み取り系。表示上は thinking
    Reading,
    /// 変更や実行を伴う。表示上は working
    Working,
    /// Discord への返信。working 扱いで、本文を吹き出しに使う
    Reply,
    /// ツール検索など補助的な呼び出し。表示上は thinking
    Auxiliary,
}

const READING_TOOLS: &[&str] = &[
    "Read",
    "Grep",
    "Glob",
    "LS",
    "WebFetch",
    "WebSearch",
    "TodoRead",
    "NotebookRead",
];

const AUXILIARY_TOOLS: &[&str] = &[
    "ToolSearch",
    "ListMcpResourcesTool",
    "ReadMcpResourceTool",
    "ReadMcpResourceDirTool",
    "TaskOutput",
    "Monitor",
    "ListAgents",
];

/// 単独では何も変更しないコマンド。
const READ_ONLY_COMMANDS: &[&str] = &[
    "cat", "ls", "head", "tail", "less", "more", "grep", "rg", "egrep", "fgrep", "find", "fd",
    "pwd", "echo", "printf", "wc", "which", "type", "stat", "file", "tree", "du", "df", "env",
    "printenv", "jq", "sort", "uniq", "cut", "awk", "tr", "diff", "cd", "true", "test", "[",
    "date", "whoami", "uname", "basename", "dirname", "realpath", "readlink", "column", "nl",
];

const GIT_READ_ONLY: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "rev-parse",
    "ls-files",
    "blame",
    "describe",
    "shortlog",
    "reflog",
    "remote",
    "branch",
    "tag",
];

/// Discord プラグインの返信ツールか。観測された名前は `mcp__plugin_discord_discord__reply`。
pub fn is_reply_tool(name: &str) -> bool {
    name.starts_with("mcp__") && name.contains("discord") && name.ends_with("__reply")
}

/// 返信ツールの本文。
pub fn reply_text(input: &Value) -> Option<String> {
    input
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub fn classify(name: &str, input: &Value) -> ToolClass {
    if is_reply_tool(name) {
        return ToolClass::Reply;
    }
    if READING_TOOLS.contains(&name) {
        return ToolClass::Reading;
    }
    if AUXILIARY_TOOLS.contains(&name) {
        return ToolClass::Auxiliary;
    }
    if name == "Bash" || name == "PowerShell" {
        let command = input.get("command").and_then(Value::as_str).unwrap_or("");
        return if shell_is_read_only(command) {
            ToolClass::Reading
        } else {
            ToolClass::Working
        };
    }
    ToolClass::Working
}

/// シェルコマンド全体が読み取りだけで済むか。
///
/// `&&`、`||`、`;`、`|`、改行で区切った各区間の先頭語を見る。
/// 出力リダイレクト(`>`)があれば書き込みとみなす。`2>&1` や `/dev/null` 行きは無視する。
pub fn shell_is_read_only(command: &str) -> bool {
    let cleaned = command
        .replace("2>&1", " ")
        .replace("&>/dev/null", " ")
        .replace("2>/dev/null", " ")
        .replace(">/dev/null", " ");
    if cleaned.contains('>') {
        return false;
    }
    let segments = cleaned
        .split(['\n', ';', '|', '&'])
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let mut any = false;
    for segment in segments {
        any = true;
        if !segment_is_read_only(segment) {
            return false;
        }
    }
    any
}

fn segment_is_read_only(segment: &str) -> bool {
    let mut words = segment
        .split_whitespace()
        .skip_while(|w| is_env_assignment(w));
    let Some(first) = words.next() else {
        return true;
    };
    let first = first.rsplit('/').next().unwrap_or(first);
    if first == "git" {
        return match words.next() {
            Some(sub) => {
                GIT_READ_ONLY.contains(&sub)
                    && !words.any(|w| w == "-d" || w == "-D" || w == "-m" || w == "-M")
            }
            None => true,
        };
    }
    READ_ONLY_COMMANDS.contains(&first)
}

fn is_env_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bash(cmd: &str) -> ToolClass {
        classify("Bash", &json!({ "command": cmd }))
    }

    #[test]
    fn read_tools_are_reading() {
        assert_eq!(classify("Read", &json!({})), ToolClass::Reading);
        assert_eq!(classify("Grep", &json!({})), ToolClass::Reading);
    }

    #[test]
    fn edit_and_agent_are_working() {
        assert_eq!(classify("Edit", &json!({})), ToolClass::Working);
        assert_eq!(classify("Write", &json!({})), ToolClass::Working);
        assert_eq!(classify("Agent", &json!({})), ToolClass::Working);
    }

    #[test]
    fn tool_search_is_auxiliary() {
        assert_eq!(classify("ToolSearch", &json!({})), ToolClass::Auxiliary);
    }

    #[test]
    fn discord_reply_is_reply() {
        assert_eq!(
            classify("mcp__plugin_discord_discord__reply", &json!({})),
            ToolClass::Reply
        );
        assert_eq!(
            reply_text(&json!({ "chat_id": "1", "text": "作りました" })).as_deref(),
            Some("作りました")
        );
    }

    #[test]
    fn other_mcp_tools_are_working() {
        assert_eq!(
            classify("mcp__pencil__execute", &json!({})),
            ToolClass::Working
        );
    }

    #[test]
    fn bash_reading_commands() {
        assert_eq!(bash("cat README.md"), ToolClass::Reading);
        assert_eq!(
            bash("ls README* 2>/dev/null; cat README.md 2>/dev/null | head -200"),
            ToolClass::Reading
        );
        assert_eq!(
            bash("cd /repo && git status && git log --oneline | head -5"),
            ToolClass::Reading
        );
        assert_eq!(bash("FOO=1 grep -rn pattern src"), ToolClass::Reading);
        assert_eq!(bash("/bin/ls -la"), ToolClass::Reading);
    }

    #[test]
    fn bash_working_commands() {
        assert_eq!(bash("touch /tmp/x && ls -la /tmp/x"), ToolClass::Working);
        assert_eq!(bash("cd /repo && cargo build"), ToolClass::Working);
        assert_eq!(bash("echo hi > file.txt"), ToolClass::Working);
        assert_eq!(bash("git push origin main"), ToolClass::Working);
        assert_eq!(bash("git branch -d old"), ToolClass::Working);
        assert_eq!(bash("python3 - <<'PY'\nprint(1)\nPY"), ToolClass::Working);
        assert_eq!(bash(""), ToolClass::Working);
    }
}
