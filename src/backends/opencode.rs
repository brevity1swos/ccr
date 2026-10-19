//! OpenCode (v2) backend.
//!
//! OpenCode v2 stores sessions in a single SQLite database at
//! `~/.local/share/opencode/opencode.db` (WAL mode, actively written by a
//! background service while the TUI runs). Instead of opening the live
//! database — which needs WAL recovery rights and takes locks that could
//! disturb the service — every read works on a throwaway snapshot copy
//! (WAL sidecar first, then the main db) that is deleted afterwards. That
//! keeps `ccr` strictly read-only at rest and correct whether or not the
//! service is running.

use anyhow::{Context, Result, bail};
use chrono::{Local, TimeZone};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::backends::Backend;
use crate::session::{
    PLACEHOLDER_TITLE, PREVIEW_TURNS, Role, Session, TITLE_MAX, Turn, append_searchable,
};
use crate::util::{OPENCODE_SESSION_FLAGS, is_possibly_live, pgrep_session_flags, truncate};

pub struct OpencodeBackend;

/// Unique suffix for snapshot dirs — `scan` runs on multiple threads
/// (`scan_all` + the TUI warm-up) and on rayon, so pid alone is not enough.
static SNAPSHOT_SEQ: AtomicU64 = AtomicU64::new(0);

impl OpencodeBackend {
    const NAME: &'static str = "opencode";

    /// Resolve OpenCode's data directory (the one containing `opencode.db`).
    ///
    /// Precedence:
    /// 1. `CCR_OPENCODE_DIR` — full path to the data dir
    /// 2. `XDG_DATA_HOME/opencode` — when `XDG_DATA_HOME` is set
    /// 3. `~/.local/share/opencode` — OpenCode's default on every platform
    fn data_dir() -> Result<PathBuf> {
        if let Ok(dir) = std::env::var("CCR_OPENCODE_DIR") {
            return Ok(PathBuf::from(dir));
        }
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
            && Path::new(&xdg).is_absolute()
        {
            return Ok(Path::new(&xdg).join("opencode"));
        }
        let home = dirs::home_dir().context("no home dir")?;
        Ok(home.join(".local").join("share").join("opencode"))
    }
}

impl Backend for OpencodeBackend {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn scan(&self) -> Result<Vec<Session>> {
        let dir = Self::data_dir()?;
        scan_dir(&dir)
    }

    /// `opencode --session <id>` continues that session (creating it only if
    /// the id does not exist, so a scanned id always resumes).
    fn resume(&self, s: &Session) -> Command {
        let mut cmd = Command::new("opencode");
        cmd.arg("--session").arg(&s.id).current_dir(&s.cwd);
        cmd
    }

    fn open_with_prompt(&self, cwd: &Path, prompt: &str) -> Command {
        let mut cmd = Command::new("opencode");
        cmd.arg("--prompt").arg(prompt).current_dir(cwd);
        cmd
    }

    /// The default guard looks for `--resume`-style argv, which OpenCode
    /// never uses; match its `--session` / `-s` forms instead. Sessions
    /// opened from inside the OpenCode TUI carry no id in argv and stay
    /// undetectable — same accepted blind spot as Gemini's index resume.
    fn running(&self, s: &Session) -> Vec<String> {
        pgrep_session_flags(&s.id, &OPENCODE_SESSION_FLAGS)
    }

    /// All turns for one session, re-read from the store by session id.
    /// (The trait's path-based [`Backend::all_turns_at`] cannot work here —
    /// see below.)
    fn all_turns(&self, s: &Session) -> Result<Vec<Turn>> {
        let snap = Snapshot::open(&s.origin)?;
        let conn = &snap.conn;
        let mut stmt = conn
            .prepare(
                "SELECT type, data FROM session_message \
                 WHERE session_id = ?1 AND type IN ('user','assistant') \
                 ORDER BY seq",
            )
            .context("prepare session_message query")?;
        let rows = stmt
            .query_map([&s.id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .context("query session_message")?;
        let mut turns = Vec::new();
        for row in rows {
            let (kind, data) = row.context("read session_message row")?;
            if let Some(t) = turn_from_row(&kind, &data) {
                turns.push(t);
            }
        }
        Ok(turns)
    }

    /// A bare path cannot say *which* session in the shared database to
    /// re-read, so the path-based re-read is unsupported for this backend.
    /// It is only used by the TUI's background full-text warm-up, which is
    /// pointless here anyway: the scan already reads the whole store in one
    /// query, so `Session.searchable` carries the (capped) full history from
    /// the start and there is nothing left to warm.
    fn all_turns_at(&self, _origin: &Path) -> Result<Vec<Turn>> {
        bail!("opencode sessions share one db; re-read via all_turns(&Session)")
    }

    /// `ccr show` dumps the raw message rows as JSONL — the db itself is not
    /// plain text, so `cat`-ing the origin file would print binary garbage.
    fn show(&self, s: &Session) -> Result<String> {
        let snap = Snapshot::open(&s.origin)?;
        let conn = &snap.conn;
        let mut stmt = conn
            .prepare(
                "SELECT id, type, seq, time_created, data FROM session_message \
                 WHERE session_id = ?1 ORDER BY seq",
            )
            .context("prepare session_message query")?;
        let rows = stmt
            .query_map([&s.id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .context("query session_message")?;
        let mut out = String::new();
        for row in rows {
            let (id, kind, seq, created, data) = row.context("read session_message row")?;
            let data: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
            let line = serde_json::json!({
                "id": id,
                "type": kind,
                "seq": seq,
                "time_created": created,
                "data": data,
            });
            out.push_str(&line.to_string());
            out.push('\n');
        }
        Ok(out)
    }
}

/// Scan one OpenCode data dir (the one containing `opencode.db`). Missing
/// dir or db is not an error — no sessions. Split from the trait method so
/// tests can pass a fixture path without touching environment variables.
fn scan_dir(dir: &Path) -> Result<Vec<Session>> {
    let db = dir.join("opencode.db");
    if !db.exists() {
        return Ok(Vec::new());
    }
    let snap = Snapshot::open(&db)?;
    let conn = &snap.conn;

    // Message rows grouped by session, seq-ordered.
    let mut turns_by_session: HashMap<String, Vec<Turn>> = HashMap::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT session_id, type, data FROM session_message \
                 WHERE type IN ('user','assistant') ORDER BY session_id, seq",
            )
            .context("prepare session_message query")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .context("query session_message")?;
        for row in rows {
            let (session_id, kind, data) = row.context("read session_message row")?;
            if let Some(t) = turn_from_row(&kind, &data) {
                turns_by_session.entry(session_id).or_default().push(t);
            }
        }
    }

    // Top-level, unarchived sessions — mirrors `opencode session list`
    // ("top-level sessions"): child sessions (forks, sub-runs) carry a
    // parent_id and archived ones were dismissed in OpenCode's own UI.
    let mut stmt = conn
        .prepare(
            "SELECT id, directory, time_updated FROM session_v2 \
             WHERE parent_id IS NULL AND time_archived IS NULL",
        )
        .context("prepare session_v2 query")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .context("query session_v2")?;

    let mut out = Vec::new();
    for row in rows {
        let (id, directory, time_updated) = row.context("read session_v2 row")?;
        let turns = turns_by_session.remove(&id).unwrap_or_default();
        out.push(build_session(
            &id,
            &directory,
            time_updated,
            &turns,
            db.clone(),
        ));
    }
    Ok(out)
}

/// Assemble a `Session` from one `session_v2` row and its seq-ordered turns.
fn build_session(
    id: &str,
    directory: &str,
    time_updated_ms: i64,
    turns: &[Turn],
    origin: PathBuf,
) -> Session {
    let cwd = if directory.is_empty() {
        PathBuf::from("(unknown)")
    } else {
        PathBuf::from(directory)
    };
    let title = turns
        .iter()
        .rev()
        .find(|t| t.role == Role::User)
        .map(|t| truncate(&t.text, TITLE_MAX))
        .unwrap_or_else(|| PLACEHOLDER_TITLE.into());
    let last_activity = Local
        .timestamp_millis_opt(time_updated_ms)
        .single()
        .unwrap_or_else(|| Local.timestamp_opt(0, 0).unwrap());

    let mut preview: VecDeque<Turn> = VecDeque::with_capacity(PREVIEW_TURNS);
    // Newest-first so the SEARCHABLE_CAP keeps the most recent turns —
    // the same "recent turns searchable" contract the file backends get
    // from their tail-window scan. (There is no warm-up tier to backfill
    // the older turns here; order inside the blob does not matter to the
    // substring filter.)
    let mut searchable = String::new();
    for t in turns.iter().rev() {
        append_searchable(&mut searchable, &t.text);
    }
    for t in turns {
        if preview.len() == PREVIEW_TURNS {
            preview.pop_front();
        }
        preview.push_back(t.clone());
    }

    Session {
        backend: OpencodeBackend::NAME,
        id: id.to_string(),
        cwd,
        title,
        last_activity,
        // The scan reads every message row anyway (one bounded query), so
        // unlike the tail-window file backends the count is exact.
        message_count: Some(turns.len()),
        preview: preview.into_iter().collect(),
        possibly_live: is_possibly_live(last_activity),
        origin,
        searchable,
    }
}

/// One user/assistant message row → `Turn`. Skips rows whose role is
/// unknown or whose visible text is empty (e.g. assistant messages that
/// carry only reasoning/tool parts).
fn turn_from_row(kind: &str, data_json: &str) -> Option<Turn> {
    let role = Role::parse(kind)?;
    let v: Value = serde_json::from_str(data_json).ok()?;
    let text = match role {
        Role::User => v
            .get("text")
            .and_then(|t| t.as_str())
            .map(String::from)
            .unwrap_or_default(),
        Role::Assistant => extract_assistant_text(&v),
    };
    if text.trim().is_empty() {
        return None;
    }
    Some(Turn { role, text })
}

/// Concatenate the `type == "text"` parts of an assistant message's content
/// array. Reasoning and tool parts are not conversation text.
fn extract_assistant_text(v: &Value) -> String {
    match v.get("content") {
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| {
                if p.get("type").and_then(|t| t.as_str()) == Some("text") {
                    p.get("text").and_then(|t| t.as_str()).map(String::from)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// A throwaway copy of the live database, opened for reading. Dropping it
/// deletes the copy; the original is never opened, locked, or written.
struct Snapshot {
    conn: Connection,
    dir: PathBuf,
}

impl Snapshot {
    fn open(db: &Path) -> Result<Self> {
        let seq = SNAPSHOT_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ccr-opencode-{}-{seq}", std::process::id()));
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        // Copy the WAL sidecar first, the main db second: if OpenCode
        // checkpoints in between, the db copy already contains the sidecar's
        // commits (replayed frames are rejected by salt/checksum) instead of
        // the db-first order, which can pair a pre-checkpoint db with a
        // reset sidecar and silently miss the newest commits.
        let file_name = db
            .file_name()
            .context("db has no file name")?
            .to_string_lossy()
            .into_owned();
        let live_wal = db.with_file_name(format!("{file_name}-wal"));
        if live_wal.exists() {
            fs::copy(&live_wal, dir.join(format!("{file_name}-wal")))
                .with_context(|| format!("copy {}", live_wal.display()))?;
        }
        fs::copy(db, dir.join(&file_name)).with_context(|| format!("copy {}", db.display()))?;
        let conn = Connection::open(dir.join(&file_name))
            .with_context(|| format!("open snapshot of {}", db.display()))?;
        Ok(Self { conn, dir })
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        // `conn` closes when the fields drop right after this body; on Unix
        // unlink-while-open is fine, so no explicit close is needed.
        fs::remove_dir_all(&self.dir).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Minimal schema with the columns the backend reads (the real db has
    /// more; the queries never touch them).
    fn create_db(dir: &Path) -> Connection {
        fs::create_dir_all(dir).unwrap();
        let conn = Connection::open(dir.join("opencode.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE session_v2 (
                id text PRIMARY KEY,
                parent_id text,
                directory text NOT NULL,
                time_archived integer,
                time_created integer NOT NULL,
                time_updated integer NOT NULL
             );
             CREATE TABLE session_message (
                id text PRIMARY KEY,
                session_id text NOT NULL,
                type text NOT NULL,
                seq integer NOT NULL,
                time_created integer NOT NULL,
                data text NOT NULL
             );",
        )
        .unwrap();
        conn
    }

    fn ins_session(conn: &Connection, id: &str, directory: &str, updated_ms: i64) {
        conn.execute(
            "INSERT INTO session_v2 (id, directory, time_created, time_updated) \
             VALUES (?1, ?2, 0, ?3)",
            (id, directory, updated_ms),
        )
        .unwrap();
    }

    fn ins_child(conn: &Connection, id: &str, parent: &str, directory: &str) {
        conn.execute(
            "INSERT INTO session_v2 (id, parent_id, directory, time_created, time_updated) \
             VALUES (?1, ?2, ?3, 0, 0)",
            (id, parent, directory),
        )
        .unwrap();
    }

    fn ins_archived(conn: &Connection, id: &str, directory: &str) {
        conn.execute(
            "INSERT INTO session_v2 (id, directory, time_archived, time_created, time_updated) \
             VALUES (?1, ?2, 123, 0, 0)",
            (id, directory),
        )
        .unwrap();
    }

    fn ins_msg(conn: &Connection, session: &str, kind: &str, seq: i64, data: Value) {
        conn.execute(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, data) \
             VALUES (?1, ?2, ?3, ?4, 0, ?5)",
            (
                format!("{session}-{seq}"),
                session,
                kind,
                seq,
                data.to_string(),
            ),
        )
        .unwrap();
    }

    fn user_msg(text: &str) -> Value {
        json!({ "text": text })
    }

    fn asst_msg(text: &str) -> Value {
        json!({ "content": [ { "type": "text", "text": text } ] })
    }

    fn one_session(dir: &Path) -> Session {
        let mut s = scan_dir(dir).expect("scan ok");
        assert_eq!(s.len(), 1, "fixture must yield exactly one session");
        s.remove(0)
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("ccr-opencode-test-{name}-{}", std::process::id()));
        fs::remove_dir_all(&d).ok();
        d
    }

    #[test]
    fn scan_reads_sessions_with_cwd_and_activity() {
        let dir = temp_dir("basic");
        {
            let conn = create_db(&dir);
            ins_session(&conn, "ses_a", "/Users/me/proj", 1_792_440_281_310);
            ins_msg(&conn, "ses_a", "user", 1, user_msg("hello"));
            ins_msg(&conn, "ses_a", "assistant", 2, asst_msg("hi back"));
        }
        let s = one_session(&dir);
        assert_eq!(s.backend, "opencode");
        assert_eq!(s.id, "ses_a");
        assert_eq!(s.cwd, PathBuf::from("/Users/me/proj"));
        assert_eq!(s.title, "hello"); // last user message
        assert_eq!(s.message_count, Some(2));
        assert_eq!(s.preview.len(), 2);
        assert_eq!(
            s.last_activity.timestamp_millis(),
            1_792_440_281_310,
            "last_activity must come from session_v2.time_updated (ms)"
        );
        assert_eq!(s.origin, dir.join("opencode.db"));
        assert!(s.searchable.contains("hi back"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn child_and_archived_sessions_are_hidden() {
        let dir = temp_dir("toplevel");
        {
            let conn = create_db(&dir);
            ins_session(&conn, "ses_top", "/p", 100);
            ins_child(&conn, "ses_kid", "ses_top", "/p");
            ins_archived(&conn, "ses_old", "/p");
            ins_msg(&conn, "ses_top", "user", 1, user_msg("top"));
            ins_msg(&conn, "ses_kid", "user", 1, user_msg("kid"));
            ins_msg(&conn, "ses_old", "user", 1, user_msg("old"));
        }
        let sessions = scan_dir(&dir).expect("scan ok");
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["ses_top"]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn assistant_turns_keep_text_parts_only() {
        let dir = temp_dir("asst");
        {
            let conn = create_db(&dir);
            ins_session(&conn, "ses_x", "/p", 0);
            ins_msg(&conn, "ses_x", "user", 1, user_msg("what?"));
            ins_msg(
                &conn,
                "ses_x",
                "assistant",
                2,
                json!({ "content": [
                    { "type": "reasoning", "text": "thinking hard" },
                    { "type": "tool", "name": "shell", "state": {} },
                    { "type": "text", "text": "part one" },
                    { "type": "text", "text": "part two" },
                ]}),
            );
            // assistant message with only reasoning/tool parts → no turn
            ins_msg(
                &conn,
                "ses_x",
                "assistant",
                3,
                json!({ "content": [ { "type": "tool", "state": {} } ] }),
            );
            // synthetic / idle rows never become turns
            ins_msg(&conn, "ses_x", "synthetic", 4, json!({ "text": "noise" }));
            ins_msg(&conn, "ses_x", "idle", 5, json!({ "text": "noise" }));
        }
        let s = one_session(&dir);
        assert_eq!(s.message_count, Some(2));
        assert_eq!(s.title, "what?");
        assert_eq!(s.preview[1].text, "part one\npart two");
        assert!(!s.searchable.contains("noise"));
        assert!(!s.searchable.contains("thinking hard"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn long_history_capped_preview_and_newest_searchable() {
        let dir = temp_dir("long");
        {
            let conn = create_db(&dir);
            ins_session(&conn, "ses_l", "/p", 0);
            for i in 0..20 {
                ins_msg(&conn, "ses_l", "user", i, user_msg(&format!("msg {i}")));
            }
        }
        let s = one_session(&dir);
        assert_eq!(s.message_count, Some(20));
        assert_eq!(s.preview.len(), PREVIEW_TURNS);
        assert_eq!(s.preview.last().unwrap().text, "msg 19");
        assert_eq!(s.title, "msg 19");
        assert!(s.searchable.contains("msg 19"));
        assert!(s.searchable.contains("msg 0"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn searchable_keeps_newest_turns_when_cap_binds() {
        let dir = temp_dir("cap");
        {
            let conn = create_db(&dir);
            ins_session(&conn, "ses_c", "/p", 0);
            ins_msg(
                &conn,
                "ses_c",
                "user",
                0,
                user_msg("OLDESTNEEDLE at the start"),
            );
            for i in 0..2_000 {
                let filler = "filler ".repeat(100);
                ins_msg(&conn, "ses_c", "user", i + 1, user_msg(&filler));
            }
            ins_msg(
                &conn,
                "ses_c",
                "user",
                2_001,
                user_msg("NEWESTNEEDLE at the end"),
            );
        }
        let s = one_session(&dir);
        assert!(s.searchable.contains("newestneedle"));
        assert!(!s.searchable.contains("oldestneedle"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn session_without_user_messages_gets_placeholder() {
        let dir = temp_dir("placeholder");
        {
            let conn = create_db(&dir);
            ins_session(&conn, "ses_p", "/p", 0);
            ins_msg(&conn, "ses_p", "assistant", 1, asst_msg("just assistant"));
        }
        let s = one_session(&dir);
        assert_eq!(s.title, PLACEHOLDER_TITLE);
        assert_eq!(s.message_count, Some(1));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_db_is_not_an_error() {
        let dir = temp_dir("missing");
        assert!(scan_dir(&dir).unwrap().is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resume_invocation_opens_session_in_cwd() {
        let s = Session {
            backend: "opencode",
            id: "ses_r".into(),
            cwd: PathBuf::from("/proj"),
            title: "t".into(),
            last_activity: Local::now(),
            message_count: Some(1),
            preview: Vec::new(),
            possibly_live: false,
            origin: PathBuf::from("<db>"),
            searchable: String::new(),
        };
        let cmd = OpencodeBackend.resume(&s);
        assert_eq!(cmd.get_program(), "opencode");
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            vec!["--session", "ses_r"]
        );
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/proj")));
    }

    #[test]
    fn open_with_prompt_uses_prompt_flag() {
        let cmd = OpencodeBackend.open_with_prompt(Path::new("/proj"), "continue this");
        assert_eq!(cmd.get_program(), "opencode");
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            vec!["--prompt", "continue this"]
        );
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/proj")));
    }

    #[test]
    fn all_turns_rereads_one_session_in_order() {
        let dir = temp_dir("allturns");
        {
            let conn = create_db(&dir);
            ins_session(&conn, "ses_1", "/p", 0);
            ins_session(&conn, "ses_2", "/p", 0);
            ins_msg(&conn, "ses_1", "user", 1, user_msg("one"));
            ins_msg(&conn, "ses_2", "user", 1, user_msg("other session"));
            ins_msg(&conn, "ses_1", "assistant", 2, asst_msg("two"));
        }
        let sessions = scan_dir(&dir).unwrap();
        let target = sessions.iter().find(|s| s.id == "ses_1").unwrap();
        let turns = OpencodeBackend.all_turns(target).unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].text, "one");
        assert_eq!(turns[1].role, Role::Assistant);
        assert_eq!(turns[1].text, "two");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn all_turns_at_bails_by_design() {
        assert!(
            OpencodeBackend
                .all_turns_at(Path::new("/x/opencode.db"))
                .is_err()
        );
    }

    #[test]
    fn show_dumps_message_rows_as_jsonl() {
        let dir = temp_dir("show");
        {
            let conn = create_db(&dir);
            ins_session(&conn, "ses_s", "/p", 0);
            ins_msg(&conn, "ses_s", "user", 1, user_msg("visible text"));
            ins_msg(&conn, "ses_s", "idle", 2, json!({ "reason": "wait" }));
        }
        let sessions = scan_dir(&dir).unwrap();
        let out = OpencodeBackend.show(&sessions[0]).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["type"], "user");
        assert_eq!(first["data"]["text"], "visible text");
        let second: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["type"], "idle"); // raw rows, not filtered turns
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn wal_snapshots_open_cleanly() {
        let dir = temp_dir("wal");
        // Hold the connection open across the scan: committed frames sit in
        // the -wal sidecar, exactly like the live service leaves them.
        let conn = create_db(&dir);
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        ins_session(&conn, "ses_w", "/p", 0);
        ins_msg(&conn, "ses_w", "user", 1, user_msg("wal row"));
        let sessions = scan_dir(&dir).expect("scan ok");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "wal row");
        drop(conn);
        fs::remove_dir_all(&dir).ok();
    }
}
