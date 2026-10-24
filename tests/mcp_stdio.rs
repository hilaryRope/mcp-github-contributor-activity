//! Drives the built binary over stdio like a real MCP client. No network: only calls
//! that are answered before any GitHub request.

use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

fn session(requests: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-github-contributor-activity"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for r in requests {
        writeln!(stdin, "{r}").unwrap();
    }
    drop(stdin); // EOF ends the server loop
    let out = child.wait_with_output().unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn handshake_list_and_validation() {
    let replies = session(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
               "params": {"name": "contributor_activity", "arguments": {"repo": "not-a-repo", "user": "x"}}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "ping"}),
    ]);
    // the notification produced no reply
    assert_eq!(replies.len(), 4);
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(replies[1]["result"]["tools"][0]["name"], "contributor_activity");
    assert_eq!(replies[1]["result"]["tools"][0]["inputSchema"]["required"], json!(["user"]));
    assert_eq!(replies[2]["result"]["isError"], true);
    assert_eq!(replies[3]["id"], 4);
}

#[test]
fn survives_garbage_lines() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-github-contributor-activity"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "not json").unwrap();
    writeln!(stdin, "{}", json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})).unwrap();
    drop(stdin);
    let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
    assert_eq!(out.lines().count(), 1);
}
