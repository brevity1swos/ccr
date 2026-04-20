# ccr — CLI Code Resume

A terminal UI session picker for CLI coding assistants. One picker across
every tool you use, with cross-tool filter, sort, and cleanup.

**Supported today:** Claude Code, Codex, Gemini CLI.

## What it does

Scans each supported tool's session store, shows every session in a TUI ranked
by last activity, and resumes the one you pick — in its original working
directory, with its original session ID, via the right CLI.

```
┌─ ccr — 211 sessions  (2 possibly live) ──────────────────────────────────┐
│ Sessions                       │ Preview                                 │
│ ▶ [claude] api-service    12m  │ tool:    claude                         │
│     fix null deref in handler  │ cwd:     ~/projects/api-service         │
│   [codex] web-app      1h ● live │ last:   2026-04-19 14:22  (12m ago)   │
│     retry on 429s then log     │ msgs:    47                             │
│   [gemini] cli-tool       3d   │ id:      a1b2c3d4-5e6f-7890-abcd-…      │
│     publish v2 release notes   │                                         │
│   [claude] docs-site      1w   │ ── recent turns ──                      │
│     rewrite getting-started    │ ❯ user                                  │
│                                │ the panic reproduces only on Windows    │
│                                │ ◆ asst                                  │
│                                │ I'll add a test for \r\n then trace…    │
└──────────────────────────────────────────────────────────────────────────┘
  ↑↓/jk · g/G top/bottom · Enter resume · d delete · D prune · / filter · ? help · q
```

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
| `/`         | filter by title, cwd, or tool           |
| `Enter`     | resume selected session (live-checked)  |
| `?` / `F1`  | help overlay                            |
| `q` / `Esc` | quit                                    |

On `Enter`, `ccr` runs `pgrep -f <session-id>` first. If a matching process is
found, a confirmation modal warns before spawning a second attachment (which
would interleave JSONL writes and corrupt the session). Otherwise it execs the
tool's resume command with the session's original `cwd`.

## Management

| Key / Command              | Action                                                 |
|----------------------------|--------------------------------------------------------|
| `d`                        | soft-delete selected session (confirm modal)           |
| `D`                        | prune-by-age modal — default `90d`, accepts `Nd/Nw/Nmo/Ny` |
| `ccr prune --older-than N` | non-interactive bulk trash; `--dry-run` to preview     |
| `ccr list`                 | plain-text dump of all sessions (tool id date title)   |

Soft-deletes go to `~/.ccr/trash/<tool>/<id>.jsonl` and are auto-pruned after
30 days. Restore by moving the file back to its original location.

## Environment variables

| Variable            | Purpose                                                    |
|---------------------|------------------------------------------------------------|
| `CCR_CLAUDE_DIR`    | Full path to Claude's `projects/` dir                      |
| `CLAUDE_CONFIG_DIR` | Claude Code's native override; `ccr` appends `projects`    |
| `CCR_CODEX_DIR`     | Full path to Codex's `sessions/` dir                       |
| `CCR_GEMINI_DIR`    | Full path to Gemini's `~/.gemini` root                     |
| `CCR_TRASH_DIR`     | Override the `~/.ccr/trash/` destination                   |

## How it works

Each backend knows where its tool stores sessions and how to resume one.

| Tool | Storage | Resume invocation |
|------|---------|-------------------|
| Claude Code | `~/.claude/projects/<encoded-cwd>/<uuid>.jsonl`         | `claude --resume <uuid>` (cwd set)   |
| Codex       | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`          | `codex resume <uuid>` (cwd set)      |
| Gemini CLI  | `~/.gemini/tmp/<project>/chats/session-*.json` + `projects.json` | `gemini --resume <N>` — ccr looks up the 1-based index at runtime via `gemini --list-sessions` |

For each session `ccr` extracts:

- `cwd` — first record with a `cwd` field
- title — first user message, truncated to 80 chars
- `last_activity` — most recent `timestamp`
- `message_count` — count of user + assistant records
- `preview` — last 6 turns, rendered in the right pane

Purely read-only. Never modifies session files.

## Adding a backend

A backend is any type that implements:

```rust
trait Backend {
    fn name(&self) -> &'static str;
    fn scan(&self) -> Result<Vec<Session>>;
    fn resume(&self, s: &Session) -> std::process::Command;
}
```

Register it in `backends::all()` and sessions surface in the shared TUI with a
tool tag in the left column.

## License

MIT
