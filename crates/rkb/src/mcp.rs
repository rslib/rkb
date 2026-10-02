//! `rkb mcp`: a Model Context Protocol server on stdin and stdout that offers the agent tools.
//! Newline-delimited JSON-RPC 2.0 with blocking I/O; see https://modelcontextprotocol.io/specification.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

/// Protocol revisions rkb speaks, newest first. A client asking for one of them gets it back.
const VERSIONS: [&str; 4] = ["2026-07-28", "2025-11-25", "2025-06-18", "2025-03-26"];

pub fn serve() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle(&line) {
            // A client that went away ends the server.
            if writeln!(out, "{reply}").and_then(|()| out.flush()).is_err() {
                break;
            }
        }
    }
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The reply to one message, or `None` for a notification.
pub fn handle(line: &str) -> Option<Value> {
    let Ok(msg) = serde_json::from_str::<Value>(line) else {
        return Some(error(Value::Null, -32700, "parse error"));
    };
    let id = msg.get("id").cloned()?;
    let params = &msg["params"];
    let result = match msg["method"].as_str().unwrap_or("") {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or("");
            let version = VERSIONS.iter().find(|v| **v == asked).unwrap_or(&VERSIONS[0]);
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "rkb", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Tools for the user's knowledge base of lessons learned. Call rkb_search before a web search or a guess about an error or problem.",
            })
        }
        "ping" => json!({}),
        // An MCP server that declares an output schema must return structured results; this one returns text.
        "tools/list" => json!({ "tools": rkb_core::tools::definitions().into_iter().map(without_output_schema).collect::<Vec<_>>() }),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let (text, exit) = crate::agent::call(name, &params["arguments"]);
            // Exit 3 is a question for the user, which the agent passes on; only 1 and 2 are failures.
            json!({ "content": [{ "type": "text", "text": text }], "isError": exit == 1 || exit == 2 })
        }
        _ => return Some(error(id, -32601, "method not found")),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn without_output_schema(mut def: Value) -> Value {
    if let Some(o) = def.as_object_mut() {
        o.remove("outputSchema");
    }
    def
}
