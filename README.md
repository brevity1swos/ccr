# ccr — CLI Code Resume

A terminal UI session picker for CLI coding assistants. Stop copy-pasting UUIDs.

Works across any CLI agent that stores sessions on disk but ships without an
interactive picker. v0.1 covers **Claude Code**; **oh-my-opencode** is next.
Tools that already ship their own picker (Codex `codex resume`, Gemini CLI
`gemini --resume`) are intentionally out of scope — use theirs.

## What it does

Scans each supported tool's session store, shows every session in a TUI ranked
by last activity, and resumes the one you pick — in its original working
directory, with its original session ID, via the right CLI.

```
┌─ ccr — 49 sessions ──────────────────────────────────────────────────────┐
│ Sessions                    │ Preview                                    │
│ ▶ rgx                 12m   │ cwd:  /Users/you/project/brevity1swos/rgx  │
│   fix parser panic on empty │ last: 2026-04-19 14:22  (12m ago)          │
│   labtalk             1h    │ msgs: 47                                   │
│   add recipe ingest flow    │ id:   154bd32d-ae8d-41f7-a7b9-...          │
│   fortune_cookie      3h    │                                            │
│   tune confetti timing      │ ── recent turns ──                         │
│                             │ ❯ user                                     │
│                             │ the panic reproduces only on windows line  │
│                             │ ◆ asst                                     │
│                             │ I'll add a test for \r\n then trace the... │
└─────────────────────────────────────────────────────────────────────────┘
  ↑↓/jk navigate · Enter resume · / filter · q quit
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

## Environment variables

| Variable            | Purpose                                                 |
|---------------------|---------------------------------------------------------|
| `CCR_CLAUDE_DIR`    | Full path to Claude's `projects/` dir (escape hatch)    |
| `CLAUDE_CONFIG_DIR` | Claude Code's native override; `ccr` appends `projects` |

Precedence: `CCR_CLAUDE_DIR` > `CLAUDE_CONFIG_DIR` > `~/.claude/projects`.

## How it works

Each backend knows where its tool stores sessions and how to resume one.

| Tool | Storage | Resume invocation |
|------|---------|-------------------|
| Claude Code   | `~/.claude/projects/<encoded-cwd>/<uuid>.jsonl`           | `claude --resume <uuid>` (cwd set)                   |
| oh-my-opencode *(planned)* | `<OPENCODE_STORAGE>/session/<project>/*.json` | `oh-my-opencode run --attach <id>` (cwd set)        |

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
