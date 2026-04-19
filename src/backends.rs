use anyhow::Result;
use std::process::Command;

use crate::session::Session;
use crate::util::pgrep_f;

pub mod claude;

pub trait Backend: Send + Sync {
    fn name(&self) -> &'static str;
    fn scan(&self) -> Result<Vec<Session>>;
    fn resume(&self, s: &Session) -> Command;

    /// List of `pid cmdline` strings for processes that appear to be attached
    /// to the given session. Default: `pgrep -af <session-id>`.
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
