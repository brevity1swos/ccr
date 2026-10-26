use anyhow::Result;
use std::process::Command;

use rayon::prelude::*;

use crate::session::{Session, Turn, append_searchable};
use crate::util::pgrep_session;
use std::path::Path;

pub mod claude;
pub mod codex;
pub mod gemini;

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
}

/// What the TUI's background warm-up learns from one full read of a session.
pub struct FullHistory {
    /// Every user + assistant turn, lowercased and capped exactly like the
    /// tail-window `Session.searchable` built during scan.
    pub searchable: String,
    /// Exact turn count — fills `Session.message_count` when the tail-window
    /// scan could not compute it.
    pub turns: usize,
}

/// Read one session file in full. `None` when the file is unreadable —
/// callers then keep the scan-time text and leave the count unknown.
///
/// Reads and parses the whole file even when the cap binds sooner; a streaming
/// early-exit would need a different `all_turns_at` contract and isn't worth it
/// for a background warm-up.
pub fn full_history(backend: &dyn Backend, origin: &Path) -> Option<FullHistory> {
    let turns = backend.all_turns_at(origin).ok()?;
    let mut searchable = String::new();
    for t in &turns {
        append_searchable(&mut searchable, &t.text);
    }
    Some(FullHistory {
        searchable,
        turns: turns.len(),
    })
}

pub fn all() -> Vec<Box<dyn Backend>> {
    vec![
        Box::new(claude::ClaudeBackend),
        Box::new(codex::CodexBackend),
        Box::new(gemini::GeminiBackend),
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
