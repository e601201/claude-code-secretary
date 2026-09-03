//! MCP(stdio, JSON-RPC 2.0 を 1 行 1 メッセージ)の最小実装。
//!
//! Claude Code が channel として扱うのに必要な範囲だけを持つ:
//! `initialize` / `ping` / `tools/list` / `tools/call`(reply)と、
//! Claude Code 拡張の `notifications/claude/channel*`。
//! 入出力を [`Value`] で扱う純粋な関数なので、プロセスを起動せずにテストできる。

use secretary_core::channel::{
    ChannelCommand, ChannelEvent, RelayedPermission, CHANNEL_SERVER_NAME,
};
use serde_json::{json, Value};

/// Claude Code が提示した版がこの中に無ければ、この版を返す。
/// (2026-07-28 を返すと channel として登録されないと公式リファレンスにある)
const PROTOCOL_FALLBACK: &str = "2025-06-18";
const SUPPORTED_PROTOCOLS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

/// モデルに読ませる説明。system prompt に入るので簡潔に。
const INSTRUCTIONS: &str = "Messages typed into the desktop secretary app arrive as <channel source=\"secretary\" chat_id=\"desktop\">. \
The user is at their desk watching the secretary character; its speech bubble holds about 120 characters. \
When you finish handling such a message, call the reply tool once with one or two short sentences in the user's language and no markdown. \
Use reply only for <channel source=\"secretary\"> messages; the secretary observes everything else on its own.";

/// 1 行を処理した結果として外へ出すもの。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outgoing {
    /// 標準出力へ書く JSON-RPC メッセージ(応答)
    ToClaude(Value),
    /// アプリへ送るフレーム
    ToApp(ChannelEvent),
}

#[derive(Debug, Default)]
pub struct Mcp;

impl Mcp {
    pub fn new() -> Self {
        Self
    }

    /// 標準入力の 1 行を処理する。
    pub fn handle_line(&mut self, line: &str) -> Vec<Outgoing> {
        let line = line.trim();
        if line.is_empty() {
            return Vec::new();
        }
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("secretary-channel: bad json from claude: {e}");
                return vec![Outgoing::ToClaude(error(
                    Value::Null,
                    -32700,
                    "parse error",
                ))];
            }
        };
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            // 自分からはリクエストを送らないので、応答が来ることはない。来ても無視する
            return Vec::new();
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        match msg.get("id") {
            Some(id) if !id.is_null() => self.request(id.clone(), method, &params),
            _ => self.notification(method, &params),
        }
    }

    fn request(&mut self, id: Value, method: &str, params: &Value) -> Vec<Outgoing> {
        match method {
            "initialize" => {
                let requested = params.get("protocolVersion").and_then(Value::as_str);
                let version = requested
                    .filter(|v| SUPPORTED_PROTOCOLS.contains(v))
                    .unwrap_or(PROTOCOL_FALLBACK);
                vec![Outgoing::ToClaude(result(
                    id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": {
                            "tools": {},
                            "experimental": {
                                "claude/channel": {},
                                // 送り手は同じ机に座っているユーザー本人(ソケットはユーザーのホーム配下、
                                // hook と同じトークンで確認)なので、権限の中継を宣言してよい
                                "claude/channel/permission": {}
                            }
                        },
                        "serverInfo": {
                            "name": CHANNEL_SERVER_NAME,
                            "version": env!("CARGO_PKG_VERSION")
                        },
                        "instructions": INSTRUCTIONS
                    }),
                ))]
            }
            "ping" => vec![Outgoing::ToClaude(result(id, json!({})))],
            "tools/list" => vec![Outgoing::ToClaude(result(
                id,
                json!({ "tools": [reply_tool_schema()] }),
            ))],
            "tools/call" => self.call_tool(id, params),
            _ => vec![Outgoing::ToClaude(error(
                id,
                -32601,
                &format!("method not found: {method}"),
            ))],
        }
    }

    fn call_tool(&mut self, id: Value, params: &Value) -> Vec<Outgoing> {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        if name != "reply" {
            return vec![Outgoing::ToClaude(result(
                id,
                json!({
                    "content": [{ "type": "text", "text": format!("unknown tool: {name}") }],
                    "isError": true
                }),
            ))];
        }
        let text = params
            .get("arguments")
            .and_then(|a| a.get("text"))
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if text.is_empty() {
            return vec![Outgoing::ToClaude(result(
                id,
                json!({
                    "content": [{ "type": "text", "text": "text is required" }],
                    "isError": true
                }),
            ))];
        }
        vec![
            Outgoing::ToApp(ChannelEvent::Reply {
                text: text.to_string(),
            }),
            Outgoing::ToClaude(result(
                id,
                json!({ "content": [{ "type": "text", "text": "sent" }] }),
            )),
        ]
    }

    fn notification(&mut self, method: &str, params: &Value) -> Vec<Outgoing> {
        match method {
            "notifications/initialized" => {
                eprintln!("secretary-channel: initialized by claude code");
                Vec::new()
            }
            "notifications/claude/channel/permission_request" => {
                let field = |k: &str| {
                    params
                        .get(k)
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string()
                };
                let request_id = field("request_id");
                if request_id.is_empty() {
                    eprintln!("secretary-channel: permission_request without request_id");
                    return Vec::new();
                }
                vec![Outgoing::ToApp(ChannelEvent::PermissionRequest(
                    RelayedPermission {
                        request_id,
                        tool_name: field("tool_name"),
                        description: field("description"),
                        input_preview: field("input_preview"),
                    },
                ))]
            }
            _ => Vec::new(),
        }
    }
}

/// アプリからの指示を Claude Code への通知に変換する。
pub fn notification_for(cmd: &ChannelCommand) -> Value {
    match cmd {
        ChannelCommand::SendPrompt { text } => json!({
            "jsonrpc": "2.0",
            "method": "notifications/claude/channel",
            "params": {
                "content": text,
                // 各キーは <channel> タグの属性になる(英数字と _ のみ)
                "meta": { "chat_id": "desktop", "ts": unix_seconds().to_string() }
            }
        }),
        ChannelCommand::RespondPermission { request_id, allow } => json!({
            "jsonrpc": "2.0",
            "method": "notifications/claude/channel/permission",
            "params": {
                "request_id": request_id,
                "behavior": if *allow { "allow" } else { "deny" }
            }
        }),
    }
}

fn reply_tool_schema() -> Value {
    json!({
        "name": "reply",
        "description": "Send a short message to the desktop secretary's speech bubble. Use it to answer a <channel source=\"secretary\"> message. Keep it under 120 characters, plain text.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "Message text (plain, short)" }
            },
            "required": ["text"]
        }
    })
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude(outs: &[Outgoing]) -> Vec<&Value> {
        outs.iter()
            .filter_map(|o| match o {
                Outgoing::ToClaude(v) => Some(v),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn initialize_declares_channel_capabilities_and_echoes_supported_version() {
        let mut mcp = Mcp::new();
        let outs = mcp.handle_line(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"claude-code","version":"2.1.259"}}}"#,
        );
        let res = &claude(&outs)[0]["result"];
        assert_eq!(res["protocolVersion"], "2025-06-18");
        assert!(res["capabilities"]["experimental"]["claude/channel"].is_object());
        assert!(res["capabilities"]["experimental"]["claude/channel/permission"].is_object());
        assert!(res["capabilities"]["tools"].is_object());
        assert_eq!(res["serverInfo"]["name"], "secretary");
        assert!(res["instructions"]
            .as_str()
            .unwrap()
            .contains("<channel source=\"secretary\""));
        assert!(mcp
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_empty());
    }

    #[test]
    fn unknown_or_future_protocol_falls_back() {
        let mut mcp = Mcp::new();
        let outs = mcp.handle_line(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-07-28"}}"#,
        );
        assert_eq!(
            claude(&outs)[0]["result"]["protocolVersion"],
            PROTOCOL_FALLBACK
        );
    }

    #[test]
    fn tools_list_exposes_reply_and_call_forwards_text_to_app() {
        let mut mcp = Mcp::new();
        let outs = mcp.handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let tools = &claude(&outs)[0]["result"]["tools"];
        assert_eq!(tools[0]["name"], "reply");
        assert_eq!(tools[0]["inputSchema"]["required"][0], "text");

        let outs = mcp.handle_line(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"reply","arguments":{"text":" できました "}}}"#,
        );
        assert_eq!(
            outs[0],
            Outgoing::ToApp(ChannelEvent::Reply {
                text: "できました".into()
            })
        );
        assert_eq!(claude(&outs)[0]["result"]["content"][0]["text"], "sent");
        assert_eq!(claude(&outs)[0]["id"], 3);
    }

    #[test]
    fn empty_reply_and_unknown_tool_are_tool_errors_not_protocol_errors() {
        let mut mcp = Mcp::new();
        let outs = mcp.handle_line(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"reply","arguments":{"text":""}}}"#,
        );
        assert_eq!(outs.len(), 1);
        assert_eq!(claude(&outs)[0]["result"]["isError"], true);
        let outs = mcp.handle_line(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
        );
        assert_eq!(claude(&outs)[0]["result"]["isError"], true);
    }

    #[test]
    fn permission_request_becomes_app_event() {
        let mut mcp = Mcp::new();
        let outs = mcp.handle_line(
            r#"{"jsonrpc":"2.0","method":"notifications/claude/channel/permission_request","params":{"request_id":"abcde","tool_name":"Bash","description":"Run tests","input_preview":"{\"command\":\"bun test\"}"}}"#,
        );
        assert_eq!(
            outs,
            vec![Outgoing::ToApp(ChannelEvent::PermissionRequest(
                RelayedPermission {
                    request_id: "abcde".into(),
                    tool_name: "Bash".into(),
                    description: "Run tests".into(),
                    input_preview: "{\"command\":\"bun test\"}".into(),
                }
            ))]
        );
    }

    #[test]
    fn ping_answers_and_unknown_request_is_method_not_found() {
        let mut mcp = Mcp::new();
        let outs = mcp.handle_line(r#"{"jsonrpc":"2.0","id":9,"method":"ping"}"#);
        assert_eq!(claude(&outs)[0]["result"], json!({}));
        let outs = mcp.handle_line(r#"{"jsonrpc":"2.0","id":10,"method":"resources/list"}"#);
        assert_eq!(claude(&outs)[0]["error"]["code"], -32601);
        // 応答や未知の通知、空行は黙って捨てる
        assert!(mcp
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#)
            .is_empty());
        assert!(mcp
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{}}"#)
            .is_empty());
        assert!(mcp.handle_line("   ").is_empty());
    }

    #[test]
    fn commands_map_to_claude_notifications() {
        let v = notification_for(&ChannelCommand::SendPrompt {
            text: "テストして".into(),
        });
        assert_eq!(v["method"], "notifications/claude/channel");
        assert_eq!(v["params"]["content"], "テストして");
        assert_eq!(v["params"]["meta"]["chat_id"], "desktop");
        assert!(v.get("id").is_none(), "通知なので id を持たない");

        let v = notification_for(&ChannelCommand::RespondPermission {
            request_id: "abcde".into(),
            allow: false,
        });
        assert_eq!(v["method"], "notifications/claude/channel/permission");
        assert_eq!(v["params"]["request_id"], "abcde");
        assert_eq!(v["params"]["behavior"], "deny");
    }
}
