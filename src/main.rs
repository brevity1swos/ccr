use anyhow::{Context, Result};

mod backends;
mod session;
mod tui;
mod util;

use backends::{all, by_name, scan_all};
use tui::{AppAction, run};

fn main() -> Result<()> {
    let backends = all();
    let sessions = scan_all(&backends);
    if sessions.is_empty() {
        eprintln!("ccr: no sessions found. Supported tools:");
        for b in &backends {
            eprintln!("  - {}", b.name());
        }
        std::process::exit(1);
    }
    match run(sessions, &backends)? {
        AppAction::Quit => Ok(()),
        AppAction::Resume(s) => {
            let backend = by_name(&backends, s.backend)
                .with_context(|| format!("unknown backend `{}`", s.backend))?;
            let status = backend
                .resume(&s)
                .status()
                .with_context(|| format!("failed to spawn `{}` — is it on PATH?", s.backend))?;
            std::process::exit(status.code().unwrap_or(1));
        }
    }
}
