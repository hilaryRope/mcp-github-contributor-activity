//! GitHub search queries and time-window bucketing.

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use std::process::Command;

const MAX_PAGES: u32 = 10;

fn token() -> Result<String, String> {
    if let Ok(t) = std::env::var("GITHUB_TOKEN") {
        if !t.is_empty() {
            return Ok(t);
        }
    }
    let out = Command::new("gh").args(["auth", "token"]).output();
    match out {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).trim().to_string()),
        _ => Err("no GitHub token: set GITHUB_TOKEN or run `gh auth login`".into()),
    }
}

/// Turns a GitHub error response into an actionable message.
fn classify_error(status: u16, message: &str, reset_epoch: Option<&str>) -> String {
    let lower = message.to_lowercase();
    match status {
        401 => "GitHub rejected the token (401): check GITHUB_TOKEN or re-run `gh auth login`".into(),
        403 | 429 if lower.contains("rate limit") || status == 429 => {
            let when = reset_epoch
                .and_then(|s| s.parse::<i64>().ok())
                .and_then(|t| DateTime::<Utc>::from_timestamp(t, 0))
                .map(|t| format!(", resets at {}", t.to_rfc3339()))
                .unwrap_or_default();
            format!("GitHub rate limit exceeded{when}")
        }
        403 => format!("GitHub denied access (403): {message}"),
        422 => format!("GitHub could not run the search (422): the user or repository may not exist or be visible to this token ({message})"),
        _ => format!("GitHub {status}: {message}"),
    }
}

/// True when GitHub reports more matches than we collected (page cap or the API's 1000-result limit).
fn is_truncated(total_count: u64, collected: usize) -> bool {
    total_count > collected as u64
}

/// Runs a search query (issues endpoint covers both issues and PRs) and returns the items
/// plus whether the result set was truncated.
fn search(client: &reqwest::blocking::Client, token: &str, q: &str) -> Result<(Vec<Value>, bool), String> {
    let mut items = Vec::new();
    let mut total_count = 0;
    for page in 1..=MAX_PAGES {
        let resp = client
            .get("https://api.github.com/search/issues")
            .query(&[("q", q), ("per_page", "100"), ("page", &page.to_string())])
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "mcp-github-contributor-activity")
            .send()
            .map_err(|e| e.to_string())?;
        let status = resp.status();
        let reset = resp
            .headers()
            .get("x-ratelimit-reset")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body: Value = resp.json().map_err(|e| e.to_string())?;
        if !status.is_success() {
            let msg = body["message"].as_str().unwrap_or("error");
            return Err(classify_error(status.as_u16(), msg, reset.as_deref()));
        }
        total_count = body["total_count"].as_u64().unwrap_or(0);
        let batch = body["items"].as_array().cloned().unwrap_or_default();
        let n = batch.len();
        items.extend(batch);
        if n < 100 {
            break;
        }
    }
    let truncated = is_truncated(total_count, items.len());
    Ok((items, truncated))
}

fn brief(item: &Value, date_field: &str) -> Value {
    json!({
        "number": item["number"],
        "title": item["title"],
        "url": item["html_url"],
        date_field: item[date_field],
    })
}

/// Splits items into day/week/month buckets by the timestamp in `field`.
fn bucket(items: &[Value], field: &str, now: DateTime<Utc>) -> Value {
    let windows = [("daily", 1), ("weekly", 7), ("monthly", 30)];
    let mut out = serde_json::Map::new();
    for (name, days) in windows {
        let cutoff = now - Duration::days(days);
        let mut hits = items
            .iter()
            .filter_map(|i| {
                let t = i[field].as_str()?.parse::<DateTime<Utc>>().ok()?;
                (t >= cutoff).then(|| (t, brief(i, field)))
            })
            .collect::<Vec<_>>();
        hits.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
        let hits: Vec<Value> = hits.into_iter().map(|(_, v)| v).collect();
        out.insert(name.into(), json!({"count": hits.len(), "items": hits}));
    }
    Value::Object(out)
}

/// Checks the inputs and returns the search scope qualifier (`repo:o/n ` or empty for all of GitHub).
fn validate(repo: Option<&str>, user: &str) -> Result<String, String> {
    if user.is_empty() || user.contains(char::is_whitespace) {
        return Err("user must be a plain GitHub login".into());
    }
    match repo {
        None => Ok(String::new()),
        Some(r) if r.split('/').count() == 2 && !r.contains(char::is_whitespace) && !r.starts_with('/') && !r.ends_with('/') => {
            Ok(format!("repo:{r} "))
        }
        Some(_) => Err("repo must be owner/name".into()),
    }
}

pub fn activity(repo: Option<&str>, user: &str) -> Result<Value, String> {
    let scope = validate(repo, user)?;
    let token = token()?;
    let client = reqwest::blocking::Client::new();
    let now = Utc::now();
    let since = (now - Duration::days(30)).format("%Y-%m-%d");

    let (merged, merged_t) = search(&client, &token, &format!("{scope}author:{user} is:pr is:merged merged:>={since}"))?;
    let (opened, opened_t) = search(&client, &token, &format!("{scope}author:{user} is:issue created:>={since}"))?;
    let (closed, closed_t) = search(&client, &token, &format!("{scope}author:{user} is:issue is:closed closed:>={since}"))?;
    let (reviewed, reviewed_t) = search(&client, &token, &format!("{scope}reviewed-by:{user} -author:{user} is:pr updated:>={since}"))?;
    let (in_flight, in_flight_t) = search(&client, &token, &format!("{scope}author:{user} is:pr is:open"))?;

    Ok(json!({
        "repo": repo,  // null = all of GitHub
        "user": user,
        "generated_at": now.to_rfc3339(),
        "windows": "rolling: daily = last 24h, weekly = last 7 days, monthly = last 30 days",
        "truncated": merged_t || opened_t || closed_t || reviewed_t || in_flight_t,
        "truncated_sections": {
            "merged_prs": merged_t, "issues_opened": opened_t,
            "issues_closed": closed_t, "prs_reviewed": reviewed_t, "in_flight_prs": in_flight_t,
        },
        "merged_prs": bucket(&merged, "closed_at", now),
        "issues_opened": bucket(&opened, "created_at", now),
        "issues_closed": bucket(&closed, "closed_at", now),
        "prs_reviewed": bucket(&reviewed, "updated_at", now),
        "in_flight_prs": {
            "count": in_flight.len(),
            "items": in_flight.iter().map(|i| json!({
                "number": i["number"], "title": i["title"], "url": i["html_url"],
                "created_at": i["created_at"], "updated_at": i["updated_at"],
            })).collect::<Vec<_>>(),
        },
        "notes": "prs_reviewed lists other authors' PRs the user reviewed, bucketed by the PR's last update (GitHub search exposes no review timestamp), so it is approximate. Issues are attributed to their author. Day-granularity search qualifiers are used, then refined by exact timestamp (merged PR time = closed_at). If `truncated` is true, GitHub had more matches than could be fetched (1000 max per query) and counts are lower bounds."
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        "2026-10-24T12:00:00Z".parse().unwrap()
    }

    fn item(n: u32, closed_at: &str) -> Value {
        json!({"number": n, "title": format!("t{n}"), "html_url": format!("u{n}"), "closed_at": closed_at})
    }

    #[test]
    fn bucket_uses_rolling_windows() {
        let items = [
            item(1, "2026-10-24T06:00:00Z"), // 6h ago: all windows
            item(2, "2026-10-20T12:00:00Z"), // 4d: weekly + monthly
            item(3, "2026-10-01T12:00:00Z"), // 23d: monthly only
            item(4, "2026-09-01T12:00:00Z"), // 53d: none
        ];
        let b = bucket(&items, "closed_at", now());
        assert_eq!(b["daily"]["count"], 1);
        assert_eq!(b["weekly"]["count"], 2);
        assert_eq!(b["monthly"]["count"], 3);
        assert_eq!(b["daily"]["items"][0]["number"], 1);
        let nums: Vec<_> = b["monthly"]["items"].as_array().unwrap().iter().map(|i| i["number"].clone()).collect();
        assert_eq!(nums, [1, 2, 3].map(|n| json!(n)));
    }

    #[test]
    fn bucket_sorts_newest_first() {
        let items = [item(1, "2026-10-10T00:00:00Z"), item(2, "2026-10-20T00:00:00Z")];
        let b = bucket(&items, "closed_at", now());
        assert_eq!(b["monthly"]["items"][0]["number"], 2);
    }

    #[test]
    fn bucket_cutoff_is_inclusive_and_skips_bad_timestamps() {
        let items = [
            item(1, "2026-10-23T12:00:00Z"), // exactly 24h
            json!({"number": 2, "closed_at": null}),
            json!({"number": 3, "closed_at": "garbage"}),
        ];
        let b = bucket(&items, "closed_at", now());
        assert_eq!(b["daily"]["count"], 1);
        assert_eq!(b["monthly"]["count"], 1);
    }

    #[test]
    fn bucket_empty() {
        let b = bucket(&[], "closed_at", now());
        assert_eq!(b["weekly"]["count"], 0);
        assert_eq!(b["weekly"]["items"], json!([]));
    }

    #[test]
    fn truncation_detection() {
        assert!(!is_truncated(0, 0));
        assert!(!is_truncated(250, 250));
        assert!(is_truncated(1500, 1000));
    }

    #[test]
    fn classify_errors() {
        assert!(classify_error(401, "Bad credentials", None).contains("token"));
        let m = classify_error(403, "API rate limit exceeded", Some("1792843200"));
        assert!(m.contains("rate limit") && m.contains("resets at 2026-10-24"));
        assert!(classify_error(429, "slow down", None).contains("rate limit"));
        assert!(classify_error(403, "Resource not accessible", None).contains("denied"));
        assert!(classify_error(422, "Validation Failed", None).contains("may not exist"));
        assert_eq!(classify_error(500, "oops", None), "GitHub 500: oops");
    }

    #[test]
    fn validate_args() {
        assert_eq!(validate(Some("owner/name"), "octocat").unwrap(), "repo:owner/name ");
        assert_eq!(validate(None, "octocat").unwrap(), "");
        assert!(validate(Some("owner"), "octocat").is_err());
        assert!(validate(Some("a/b/c"), "octocat").is_err());
        assert!(validate(Some("owner/"), "octocat").is_err());
        assert!(validate(Some("owner/name"), "").is_err());
        assert!(validate(Some("owner/name"), "a is:pr").is_err());
        assert!(validate(Some("owner/na\tme"), "octocat").is_err());
    }
}
