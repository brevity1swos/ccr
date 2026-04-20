use anyhow::{Context, Result};
use chrono::Local;
use clap::{Parser, Subcommand};

mod age;
mod backends;
mod session;
mod trash;
mod tui;
mod util;

use backends::{Backend, all, by_name, scan_all};
use session::Session;
use tui::{AppAction, run};
use util::truncate;

#[derive(Parser)]
#[command(name = "ccr", version, about = "CLI Code Resume — TUI session picker")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Move sessions older than a given age to the ccr trash directory.
    Prune {
        /// Age threshold. Accepts Nd, Nw, Nmo, Ny, or a bare number (days).
        #[arg(long, value_name = "AGE")]
        older_than: String,
        /// Show what would be pruned without moving anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// List all sessions as plain text (tool id date title).
    List,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let _ = trash::auto_prune();

    match cli.command {
        None => launch_picker(),
        Some(Command::Prune {
            older_than,
            dry_run,
        }) => run_prune(&older_than, dry_run),
        Some(Command::List) => run_list(),
    }
}

fn launch_picker() -> Result<()> {
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

fn run_list() -> Result<()> {
    let backends = all();
    for s in scan_all(&backends) {
        println!(
            "[{}] {}  {}  {}",
            s.backend,
            s.id,
            s.last_activity.format("%Y-%m-%d %H:%M"),
            truncate(&s.title, 60)
        );
    }
    Ok(())
}

fn run_prune(age_str: &str, dry_run: bool) -> Result<()> {
    let Some(age) = age::parse_age(age_str) else {
        anyhow::bail!("invalid --older-than `{age_str}` (use forms like 30d, 2w, 3mo, 1y)")
    };
    let threshold = Local::now() - age;
    let backends = all();
    let sessions = scan_all(&backends);
    let stale: Vec<Session> = sessions
        .into_iter()
        .filter(|s| s.last_activity < threshold)
        .collect();

    if stale.is_empty() {
        println!("ccr: no sessions older than {age_str}");
        return Ok(());
    }

    println!("{} session(s) older than {age_str}:", stale.len());
    for s in &stale {
        println!(
            "  [{}] {}  {}  {}",
            s.backend,
            s.id,
            s.last_activity.format("%Y-%m-%d"),
            truncate(&s.title, 60)
        );
    }

    if dry_run {
        println!("(dry-run — nothing moved)");
        return Ok(());
    }

    let (ok, fail) = trash_sessions(&backends, &stale);
    let dest = trash::trash_root()?;
    println!("moved {ok} session(s) to {}", dest.display());
    if fail > 0 {
        eprintln!("{fail} failed — see stderr above");
    }
    Ok(())
}

pub(crate) fn trash_sessions(
    backends: &[Box<dyn Backend>],
    sessions: &[Session],
) -> (usize, usize) {
    let mut ok = 0;
    let mut fail = 0;
    for s in sessions {
        let Some(backend) = by_name(backends, s.backend) else {
            eprintln!("ccr: no backend for `{}`", s.backend);
            fail += 1;
            continue;
        };
        match backend.trash(s) {
            Ok(()) => ok += 1,
            Err(e) => {
                eprintln!("ccr: trash failed for {}: {e}", s.id);
                fail += 1;
            }
        }
    }
    (ok, fail)
}
