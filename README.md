# mcp-github-contributor-activity

A stdio [MCP](https://modelcontextprotocol.io) server with one read-only tool, `contributor_activity`.
For one GitHub user, in one repository or across all of GitHub, it reports, over rolling day / 7-day / 30-day windows:

- merged PRs
- issues opened and issues closed
- PRs the user reviewed (approximate: bucketed by the PR's last update)
- currently open PRs

Results carry a `truncated` flag (and per-section `truncated_sections`) when GitHub had more
matches than the 1000 its search API will return; counts are then lower bounds.

## Setup

```sh
cargo build --release
claude mcp add contributor-activity -- "$PWD/target/release/mcp-github-contributor-activity"
```

Authentication: set `GITHUB_TOKEN`, or log in with `gh auth login` (the server falls back to `gh auth token`).

## Tool

`contributor_activity(user: "login", repo?: "owner/name")` — omit `repo` to search all of GitHub (more likely to hit the truncation limit).

## Development

```sh
cargo test
```
