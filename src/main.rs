//! MCP server (stdio, JSON-RPC 2.0) reporting a contributor's GitHub activity
//! over daily / weekly / monthly windows.

mod github;

use serde_json::{Value, json};
use std::io::{BufRead, Write};

fn tool_schema() -> Value {
    json!({
        "name": "contributor_activity",
        "description": "For one GitHub user (in one repository, or across all of GitHub when `repo` is omitted), report merged PRs, issues opened, issues closed and in-flight (open) PRs over the last day, week (7d) and month (30d), plus PRs they reviewed. Results include a `truncated` flag when GitHub had more matches than could be fetched.",
        "annotations": {"title": "Contributor activity", "readOnlyHint": true, "openWorldHint": true},
        "inputSchema": {
            "type": "object",
            "properties": {
                "repo": {"type": "string", "description": "Repository as owner/name. Omit to search across all of GitHub."},
                "user": {"type": "string", "description": "GitHub login"},
                "days": {"type": "integer", "minimum": 1, "maximum": 365, "description": "Optional extra lookback window in days, reported as `last_<days>d` alongside daily/weekly/monthly. Large ranges are more likely to be truncated."}
            },
            "required": ["user"]
        }
    })
}

fn call_tool(params: &Value) -> Value {
    let name = params["name"].as_str().unwrap_or("");
    let args = &params["arguments"];
    let result = if name == "contributor_activity" {
        match args["user"].as_str() {
            Some(user) => {
                let days = match &args["days"] {
                    Value::Null => None,
                    v => match v.as_i64() {
                        Some(d) => Some(d),
                        None => return tool_error("`days` must be an integer".to_string()),
                    },
                };
                github::activity(args["repo"].as_str().filter(|r| !r.is_empty()), user, days)
            },
            None => Err("`user` is required".to_string()),
        }
    } else {
        Err(format!("unknown tool: {name}"))
    };
    match result {
        Ok(v) => json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap()}]}),
        Err(e) => tool_error(e),
    }
}

fn tool_error(text: String) -> Value {
    json!({"isError": true, "content": [{"type": "text", "text": text}]})
}

fn handle(req: &Value) -> Option<Value> {
    let id = req.get("id")?.clone(); // notifications have no id: no reply
    let result = match req["method"].as_str().unwrap_or("") {
        "initialize" => json!({
            "protocolVersion": req["params"]["protocolVersion"].as_str().unwrap_or("2025-03-26"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "mcp-github-contributor-activity", "version": env!("CARGO_PKG_VERSION")}
        }),
        "ping" => json!({}),
        "tools/list" => json!({"tools": [tool_schema()]}),
        "tools/call" => call_tool(&req["params"]),
        m => {
            return Some(json!({"jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": format!("method not found: {m}")}}));
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn main() {
    let stdout = std::io::stdout();
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(resp) = handle(&req) {
            let mut out = stdout.lock();
            let _ = writeln!(out, "{resp}");
            let _ = out.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_get_no_reply() {
        assert!(handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
    }

    #[test]
    fn tools_list_exposes_tool() {
        let r = handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).unwrap();
        assert_eq!(r["result"]["tools"][0]["name"], "contributor_activity");
    }

    #[test]
    fn unknown_method_errors() {
        let r = handle(&json!({"jsonrpc": "2.0", "id": 7, "method": "nope"})).unwrap();
        assert_eq!(r["id"], 7);
        assert_eq!(r["error"]["code"], -32601);
    }

    #[test]
    fn call_tool_validates_arguments() {
        let r = call_tool(&json!({"name": "contributor_activity", "arguments": {"repo": "a/b"}}));
        assert_eq!(r["isError"], true);
        let r = call_tool(&json!({"name": "contributor_activity", "arguments": {"repo": "bad", "user": "x"}}));
        assert_eq!(r["isError"], true);
        let r = call_tool(&json!({"name": "contributor_activity", "arguments": {"user": "x", "days": "ten"}}));
        assert_eq!(r["isError"], true);
        let r = call_tool(&json!({"name": "contributor_activity", "arguments": {"user": "x", "days": 0}}));
        assert_eq!(r["isError"], true);
        let r = call_tool(&json!({"name": "other", "arguments": {}}));
        assert_eq!(r["isError"], true);
    }
}
