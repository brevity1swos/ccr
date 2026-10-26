use anyhow::{Context, Result};
use std::process::Command;

use rayon::prelude::*;

use crate::session::{Session, Turn, append_searchable};
use crate::util::pgrep_session;
use std::path::Path;

pub mod claude;
pub mod codex;
pub mod gemini;
pub mod opencode;

/// One supported CLI coding assistant. Implementations know how to list the
/// tool's disk-backed sessions and how to resume one of them.
pub trait Backend: Send + Sync {
    /// Short tool identifier used in the TUI tag and for registry lookup
    /// (`by_name`). Must be unique across registered backends.
    fn name(&self) -> &'static str;

    /// Walk the tool's session store and return one `Session` per resumable
    /// conversation. Must not panic if the store is missing — return
    /// `Ok(Vec::new())` for that case.
    fn scan(&self) -> Result<Vec<Session>>;

    /// Build (but do not spawn) the command that resumes the given session.
    /// The caller sets `cwd` expectations via `Command::current_dir`.
    fn resume(&self, s: &Session) -> Command;

    /// `pid cmdline` strings for processes that appear to be attached to this
    /// session. Empty if none. Used to warn before resuming a live session.
    /// Default matches processes carrying the id as a resume argument
    /// (`--resume <id>` / `-r <id>` / `resume <id>`); override when the tool's CLI does not
    /// embed the session ID in its resume argv (e.g. Gemini's index-based resume).
    fn running(&self, s: &Session) -> Vec<String> {
        pgrep_session(&s.id)
    }

    /// All user + assistant turns from the session file at `origin`, in
    /// chronological order. Re-reads the file (not capped like
    /// `Session.preview`). Used by `ccr export` and the full-history search
    /// warm-up; takes a bare path so background threads need no `Session`.
    fn all_turns_at(&self, origin: &Path) -> Result<Vec<Turn>>;

    /// Convenience wrapper over [`Backend::all_turns_at`].
    fn all_turns(&self, s: &Session) -> Result<Vec<Turn>> {
        self.all_turns_at(&s.origin)
    }

    /// Raw dump for `ccr show`. Default: the text file at `origin`. Override
    /// when the store is not a plain per-session file (e.g. OpenCode's
    /// shared SQLite db, whose binary bytes must not hit the terminal).
    fn show(&self, s: &Session) -> Result<String> {
        std::fs::read_to_string(&s.origin).with_context(|| format!("read {}", s.origin.display()))
    }
}

/// Full-history searchable text for one session file: every user + assistant
/// turn, lowercased and capped exactly like the tail-window `Session.searchable`
/// built during scan. Empty when the file is unreadable — callers should then
/// fall back to the scan-time text rather than caching the empty result.
///
/// Reads and parses the whole file even when the cap binds sooner; a streaming
/// early-exit would need a different `all_turns_at` contract and isn't worth it
/// for a background warm-up.
pub fn full_searchable(backend: &dyn Backend, origin: &Path) -> String {
    let mut buf = String::new();
    if let Ok(turns) = backend.all_turns_at(origin) {
        for t in &turns {
            append_searchable(&mut buf, &t.text);
        }
    }
    buf
}

pub fn all() -> Vec<Box<dyn Backend>> {
    vec![
        Box::new(claude::ClaudeBackend),
        Box::new(codex::CodexBackend),
        Box::new(gemini::GeminiBackend),
        Box::new(opencode::OpencodeBackend),
    ]
}

/// Scan all backends in parallel, merge results sorted by last activity.
pub fn scan_all(backends: &[Box<dyn Backend>]) -> Vec<Session> {
    let mut out: Vec<Session> = backends
        .par_iter()
        .flat_map(|b| match b.scan() {
            Ok(sessions) => sessions,
            Err(e) => {
                eprintln!("ccr: {} backend scan failed: {e}", b.name());
                Vec::new()
            }
        })
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.last_activity));
    out
}

pub fn by_name<'a>(backends: &'a [Box<dyn Backend>], name: &str) -> Option<&'a dyn Backend> {
    backends
        .iter()
        .find(|b| b.name() == name)
        .map(|b| b.as_ref())
}
