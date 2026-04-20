use anyhow::Result;
use std::process::Command;

use crate::session::Session;
use crate::util::pgrep_f;

pub mod claude;

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
    /// Default scans `pgrep -af <session-id>`; override when the tool's CLI
    /// does not embed the session ID in its argv.
    fn running(&self, s: &Session) -> Vec<String> {
        pgrep_f(&s.id)
    }
}

pub fn all() -> Vec<Box<dyn Backend>> {
    vec![Box::new(claude::ClaudeBackend)]
}

pub fn scan_all(backends: &[Box<dyn Backend>]) -> Vec<Session> {
    let mut out = Vec::new();
    for b in backends {
        match b.scan() {
            Ok(sessions) => out.extend(sessions),
            Err(e) => eprintln!("ccr: {} backend scan failed: {e}", b.name()),
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.last_activity));
    out
}

pub fn by_name<'a>(backends: &'a [Box<dyn Backend>], name: &str) -> Option<&'a dyn Backend> {
    backends
        .iter()
        .find(|b| b.name() == name)
        .map(|b| b.as_ref())
}
