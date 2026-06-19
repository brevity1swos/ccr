<h1 align="center">ccr — CLI Code Resume</h1>

<p align="center">
  <em>One TUI session picker across every CLI coding assistant you use.</em>
</p>

<p align="center">
  <a href="https://crates.io/crates/ccr"><img src="https://img.shields.io/crates/v/ccr?style=flat-square&labelColor=black&color=orange" alt="crates.io"></a>
  <a href="https://github.com/brevity1swos/ccr/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/brevity1swos/ccr/ci.yml?branch=main&style=flat-square&label=CI&labelColor=black" alt="CI"></a>
  <a href="https://crates.io/crates/ccr"><img src="https://img.shields.io/crates/d/ccr?style=flat-square&labelColor=black" alt="Downloads"></a>
  <a href="./LICENSE"><img src="https://img.shields.io/crates/l/ccr?style=flat-square&labelColor=black" alt="License"></a>
  <a href="./Cargo.toml"><img src="https://img.shields.io/badge/MSRV-1.88-informational?style=flat-square&labelColor=black" alt="MSRV"></a>
</p>

<p align="center">
  <strong>Supported:</strong> Claude Code · Codex · Gemini CLI
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/brevity1swos/ccr/main/assets/demo.gif" alt="ccr demo — multi-tool session picker, nickname, filter" width="100%">
</p>

## What it does

Scans every supported tool's session store, ranks each session by last
activity, and resumes the one you pick — in its original working directory,
with its original session ID, via the right CLI.

Each row's subtitle is the **last** user message in that session, so you see
what you were working on most recently — not how the chat opened. Press `n`
on any row to add a yellow nickname; nicknamed rows expand to a third line
that keeps the auto-derived title visible (dim) for context, and the
nickname is searched by `/` too.

```
┌─ ccr — 211 sessions  (2 possibly live) ──────────────────────────────────┐
│ Sessions                              │ Preview                          │
│ ▶ [claude] api-service        12m ago │ nick:    panic hotfix            │
│     panic hotfix                      │ tool:    claude                  │
│     actually let's also add a regr…   │ cwd:     ~/projects/api-service  │
│   [codex]  web-app         1h ● live  │ last:    2026-05-23 14:00  (12m) │
│     use exponential backoff capped…   │ msgs:    47                      │
│   [gemini] cli-tool             3d    │ id:      a1b2c3d4-5e6f-…         │
│     update the migration guide se…    │                                  │
│   [claude] docs-site            1w    │ ── recent turns ──               │
│     tighten the intro paragraph a…    │ ❯ user                           │
│                                       │ the panic reproduces only when…  │
│                                       │ ◆ asst                           │
│                                       │ Let me add a None check in the…  │
└──────────────────────────────────────────────────────────────────────────┘
  ↑↓/jk · Enter resume · b bookmark · n nickname · / filter · ? help · q
```

Top row is **nicknamed** — `panic hotfix` shows in yellow, the auto-derived
last-message title stays visible underneath in dim gray. Rows without a
nickname are two lines (tags + auto-title), the same as before.

If a selected session is already running elsewhere (detected via
`pgrep -f <session-id>`), a confirmation modal appears before spawning a
second attachment:

```
        ┌─ Confirm resume ──────────────────────────────────────┐
        │ ⚠  Session may already be running                     │
        │                                                       │
        │ tool:    claude                                       │
        │ session: a1b2c3d4-5e6f-7890-abcd-…                    │
        │ cwd:     ~/projects/web-app                           │
        │                                                       │
        │ matching processes:                                   │
        │ 42318 claude --resume a1b2c3d4-5e6f-…                 │
        │                                                       │
        │ Resuming may interleave writes and corrupt session.   │
        │                                                       │
        │ [y] resume anyway    [n] cancel                       │
        └───────────────────────────────────────────────────────┘
```

## Install

```sh
cargo install ccr
```

Or from source:

```sh
cargo install --path .
```

## Use

```sh
ccr
```

| Key         | Action                                  |
|-------------|-----------------------------------------|
| `↑` / `k`   | up                                      |
| `↓` / `j`   | down                                    |
| `g` / `Home`| jump to top                             |
| `G` / `End` | jump to bottom                          |
| `PgUp/Dn`   | page up/down (10 rows)                  |
| `/`         | filter: title, cwd, tool, nickname, or content (recent turns only) |
| `Enter`     | resume selected session (live-checked)  |
| `v`         | open session in [agx] (timeline viewer) |
| `b`         | toggle bookmark (★ marker, persisted)   |
| `n`         | set / edit / clear session nickname (yellow label) |
| `?` / `F1`  | help overlay                            |
| `q` / `Esc` | quit                                    |

[agx]: https://github.com/brevity1swos/agx

On `Enter`, `ccr` runs `pgrep -f <session-id>` first. If a matching process is
found, a confirmation modal warns before spawning a second attachment (which
would interleave JSONL writes and corrupt the session). Otherwise it execs the
tool's resume command with the session's original `cwd`.

## CLI subcommands

| Command              | Action                                                 |
|----------------------|--------------------------------------------------------|
| `ccr list`           | plain-text dump of all sessions (tool id date title)   |
| `ccr path <id>`      | absolute path to session file (pipes well)             |
| `ccr show <id>`      | raw file contents (same as `cat $(ccr path <id>)`)     |
| `ccr export <id>`    | full-turn markdown dump (or `--format json`)           |
| `ccr stats`          | totals, per-tool, per-project, 30-day activity histogram |

`ccr` never modifies your session files. If you want to delete one, delete
it where its tool stored it (`~/.claude/projects/...`, `~/.codex/sessions/...`,
or `~/.gemini/tmp/...`).

## Environment variables

| Variable              | Purpose                                                    |
|-----------------------|------------------------------------------------------------|
| `CCR_CLAUDE_DIR`      | Full path to Claude's `projects/` dir                      |
| `CLAUDE_CONFIG_DIR`   | Claude Code's native override; `ccr` appends `projects`    |
| `CCR_CODEX_DIR`       | Full path to Codex's `sessions/` dir                       |
| `CCR_GEMINI_DIR`      | Full path to Gemini's `~/.gemini` root                     |
| `CCR_BOOKMARKS_FILE`  | Override the `~/.ccr/bookmarks.json` location              |
| `CCR_NICKNAMES_FILE`  | Override the `~/.ccr/nicknames.json` location              |

## How it works

Each backend knows where its tool stores sessions and how to resume one.

| Tool | Storage | Resume invocation |
|------|---------|-------------------|
| Claude Code | `~/.claude/projects/<encoded-cwd>/<uuid>.jsonl`         | `claude --resume <uuid>` (cwd set)   |
| Codex       | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`          | `codex resume <uuid>` (cwd set)      |
| Gemini CLI  | `~/.gemini/tmp/<project>/chats/session-*.json` + `projects.json` | `gemini --resume <N>` — ccr looks up the 1-based index at runtime via `gemini --list-sessions` |

For each session `ccr` extracts:

- `cwd` — working directory the session was started in
- title — **last** user message in the session, truncated to 80 chars (so the
  list shows what you were working on most recently, not how the chat opened)
- `last_activity` — most recent turn timestamp (drives the sort and the `● live` flag)
- `message_count` — user + assistant turns only
- `preview` — last 6 turns, rendered in the right pane

Read-only at rest. `ccr` never touches your session files. The only state it
writes is `~/.ccr/bookmarks.json` (when you press `b`) and
`~/.ccr/nicknames.json` (when you press `n`). Both are plain JSON.

## Adding a backend

A backend is any type that implements:

```rust
pub trait Backend: Send + Sync {
    fn name(&self) -> &'static str;
    fn scan(&self) -> Result<Vec<Session>>;
    fn resume(&self, s: &Session) -> std::process::Command;
    fn all_turns(&self, s: &Session) -> Result<Vec<Turn>>;  // for ccr export

    // Optional override with a useful default:
    fn running(&self, s: &Session) -> Vec<String>;   // default: pgrep -f <id>
}
```

Register it in `backends::all()` and sessions surface in the shared TUI with a
`[tool]` tag in the left column.

## Links

- [crates.io/crates/ccr](https://crates.io/crates/ccr)
- [docs.rs/ccr](https://docs.rs/ccr)
- [Changelog](./CHANGELOG.md)

## License

MIT
