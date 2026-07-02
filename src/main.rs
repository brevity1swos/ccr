use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

mod backends;
mod bookmarks;
mod nicknames;
mod session;
mod tail;
mod tui;
mod util;

use backends::{Backend, all, by_name, scan_all};
use session::{Role, Session, Turn};
use tui::{AppAction, run};
use util::truncate;

#[derive(Copy, Clone, Debug, clap::ValueEnum)]
enum ExportFormat {
    Md,
    Json,
}

#[derive(Parser)]
#[command(name = "ccr", version, about = "CLI Code Resume — TUI session picker")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Resume a session directly by id, skipping the picker
    /// (the CLI equivalent of selecting it in the TUI).
    Resume {
        /// Session id.
        id: String,
        /// Resume even if the session appears to be running elsewhere.
        #[arg(short, long)]
        force: bool,
    },
    /// List all sessions as plain text (tool id date title).
    List,
    /// Print the absolute path to a session's on-disk file.
    /// Useful in shell pipelines: `cat $(ccr path <id>)`.
    Path {
        /// Session id.
        id: String,
    },
    /// Print a session's raw file contents (equivalent to `cat $(ccr path …)`).
    Show {
        /// Session id.
        id: String,
    },
    /// Export a session as markdown (default) or JSON — full turns, not just preview.
    Export {
        /// Session id.
        id: String,
        /// Output format.
        #[arg(long, value_enum, default_value_t = ExportFormat::Md)]
        format: ExportFormat,
    },
    /// Activity overview — totals, per-tool, per-project, per-day histogram.
    Stats,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => launch_picker(),
        Some(Command::Resume { id, force }) => run_resume(&id, force),
        Some(Command::List) => run_list(),
        Some(Command::Path { id }) => run_path(&id),
        Some(Command::Show { id }) => run_show(&id),
        Some(Command::Export { id, format }) => run_export(&id, format),
        Some(Command::Stats) => run_stats(),
    }
}

fn find_session_by_id(id: &str) -> Result<Session> {
    let backends = all();
    find_session_and_backend(&backends, id).map(|(_, session)| session)
}

fn run_path(id: &str) -> Result<()> {
    let s = find_session_by_id(id)?;
    println!("{}", s.origin.display());
    Ok(())
}

fn run_show(id: &str) -> Result<()> {
    let s = find_session_by_id(id)?;
    let content = std::fs::read_to_string(&s.origin)
        .with_context(|| format!("read {}", s.origin.display()))?;
    print!("{content}");
    Ok(())
}

/// Resolve a session id to its owning backend and parsed session, scanning every
/// store. Shared by `resume` and `export`, which both need the backend handle.
fn find_session_and_backend<'a>(
    backends: &'a [Box<dyn Backend>],
    id: &str,
) -> Result<(&'a dyn Backend, Session)> {
    let session = scan_all(backends)
        .into_iter()
        .find(|s| s.id == id)
        .with_context(|| format!("no session with id `{id}`"))?;
    let backend = by_name(backends, session.backend)
        .with_context(|| format!("unknown backend `{}`", session.backend))?;
    Ok((backend, session))
}

/// Resume a session by id without the picker. Refuses a session that looks live
/// (another process has it open) unless `force`, mirroring the TUI's confirm
/// modal — a second attachment interleaves JSONL writes and corrupts the session.
fn run_resume(id: &str, force: bool) -> Result<()> {
    let backends = all();
    let (backend, session) = find_session_and_backend(&backends, id)?;
    if !force {
        let running = backend.running(&session);
        if !running.is_empty() {
            eprintln!("ccr: session {id} may already be running:");
            for p in &running {
                eprintln!("  {p}");
            }
            eprintln!("Resuming may interleave writes and corrupt the session.");
            eprintln!("Re-run with `--force` to resume anyway.");
            std::process::exit(1);
        }
    }
    let status = backend
        .resume(&session)
        .status()
        .with_context(|| format!("failed to spawn `{}` — is it on PATH?", session.backend))?;
    std::process::exit(status.code().unwrap_or(1));
}

fn run_export(id: &str, format: ExportFormat) -> Result<()> {
    let backends = all();
    let (backend, session) = find_session_and_backend(&backends, id)?;
    let turns = backend.all_turns(&session)?;
    match format {
        ExportFormat::Md => print!("{}", format_md(&session, &turns)),
        ExportFormat::Json => println!("{}", format_json(&session, &turns)?),
    }
    Ok(())
}

pub(crate) fn format_md(s: &Session, turns: &[Turn]) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Session `{}`\n\n", s.id));
    out.push_str(&format!("- **Tool:** {}\n", s.backend));
    out.push_str(&format!(
        "- **Last active:** {}\n",
        s.last_activity.format("%Y-%m-%d %H:%M")
    ));
    out.push_str(&format!("- **cwd:** `{}`\n", s.cwd.display()));
    out.push_str(&format!("- **Turns:** {}\n\n", turns.len()));
    out.push_str("---\n\n");
    for t in turns {
        let tag = match t.role {
            Role::User => "## ❯ user",
            Role::Assistant => "## ◆ assistant",
        };
        out.push_str(tag);
        out.push_str("\n\n");
        out.push_str(t.text.trim());
        out.push_str("\n\n");
    }
    out
}

/// Replace any `None` message_count with a freshly computed value. The counter
/// is injected so the hot path stays testable without touching the filesystem.
fn fill_counts(sessions: &mut [Session], count: impl Fn(&Session) -> usize) {
    for s in sessions.iter_mut() {
        if s.message_count.is_none() {
            s.message_count = Some(count(s));
        }
    }
}

fn run_stats() -> Result<()> {
    let backends = all();
    let mut sessions = scan_all(&backends);
    fill_counts(&mut sessions, |s| {
        by_name(&backends, s.backend)
            .and_then(|b| b.all_turns(s).ok())
            .map(|t| t.len())
            .unwrap_or(0)
    });
    print!("{}", format_stats(&sessions));
    Ok(())
}

pub(crate) fn format_stats(sessions: &[Session]) -> String {
    use std::collections::BTreeMap;
    use std::collections::HashMap;

    let mut out = String::new();
    let total = sessions.len();
    let total_turns: usize = sessions.iter().filter_map(|s| s.message_count).sum();
    let tools: std::collections::BTreeSet<&str> = sessions.iter().map(|s| s.backend).collect();

    out.push_str(&format!(
        "Total: {total} session{}  ·  {total_turns} turn{}  ·  {} tool{}\n",
        plural(total),
        plural(total_turns),
        tools.len(),
        plural(tools.len()),
    ));

    let mut by_tool: HashMap<&str, (usize, usize)> = HashMap::new();
    for s in sessions {
        let e = by_tool.entry(s.backend).or_default();
        e.0 += 1;
        e.1 += s.message_count.unwrap_or(0);
    }
    out.push_str("\nBy tool:\n");
    let mut tool_rows: Vec<_> = by_tool.iter().collect();
    tool_rows.sort_by_key(|(_, (count, _))| std::cmp::Reverse(*count));
    for (tool, (count, turns)) in tool_rows {
        out.push_str(&format!(
            "  {tool:<10} {count:>5} session{}  {turns:>7} turn{}\n",
            plural(*count),
            plural(*turns)
        ));
    }

    let mut by_project: HashMap<String, usize> = HashMap::new();
    for s in sessions {
        let name = s
            .cwd
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("(unknown)")
            .to_string();
        *by_project.entry(name).or_default() += 1;
    }
    let mut projects: Vec<_> = by_project.into_iter().collect();
    projects.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    let top = projects.len().min(10);
    out.push_str(&format!("\nBy project (top {top}):\n"));
    for (name, count) in projects.iter().take(top) {
        out.push_str(&format!(
            "  {name:<30} {count:>5} session{}\n",
            plural(*count)
        ));
    }

    let now = chrono::Local::now().date_naive();
    let mut by_date: BTreeMap<chrono::NaiveDate, usize> = BTreeMap::new();
    for s in sessions {
        let d = s.last_activity.date_naive();
        if (now - d).num_days() < 30 {
            *by_date.entry(d).or_default() += 1;
        }
    }
    if !by_date.is_empty() {
        out.push_str("\nActivity (last 30 days):\n");
        let max = by_date.values().copied().max().unwrap_or(1).max(1);
        for (date, count) in by_date.iter().rev() {
            let width = (count * 30 + max / 2) / max;
            let bar = "▇".repeat(width.max(1));
            out.push_str(&format!("  {date}  {bar} {count}\n"));
        }
    }

    let live = sessions.iter().filter(|s| s.possibly_live).count();
    if live > 0 {
        out.push_str(&format!(
            "\nPossibly live: {live} session{} (active in last 5 min)\n",
            plural(live),
        ));
    }

    out
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

pub(crate) fn format_json(s: &Session, turns: &[Turn]) -> Result<String> {
    let turns_json: Vec<serde_json::Value> = turns
        .iter()
        .map(|t| {
            serde_json::json!({
                "role": match t.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                },
                "text": &t.text,
            })
        })
        .collect();
    let doc = serde_json::json!({
        "id": s.id,
        "backend": s.backend,
        "cwd": s.cwd.to_string_lossy(),
        "last_activity": s.last_activity.to_rfc3339(),
        "message_count": turns.len(),
        "turns": turns_json,
    });
    Ok(serde_json::to_string_pretty(&doc)?)
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
        AppAction::View(s) => {
            let status = std::process::Command::new("agx")
                .arg(&s.origin)
                .current_dir(&s.cwd)
                .status()
                .context(
                    "failed to spawn `agx` — install from https://github.com/brevity1swos/agx",
                )?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;
    use clap::Parser;
    use std::path::PathBuf;

    #[test]
    fn cli_parses_resume_subcommand() {
        let cli = Cli::try_parse_from(["ccr", "resume", "abc-123"]).unwrap();
        match cli.command {
            Some(Command::Resume { id, force }) => {
                assert_eq!(id, "abc-123");
                assert!(!force);
            }
            _ => panic!("expected resume subcommand"),
        }
    }

    #[test]
    fn cli_parses_resume_force_flag() {
        for args in [
            ["ccr", "resume", "--force", "abc"],
            ["ccr", "resume", "-f", "abc"],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            match cli.command {
                Some(Command::Resume { id, force }) => {
                    assert_eq!(id, "abc");
                    assert!(force);
                }
                _ => panic!("expected resume subcommand"),
            }
        }
    }

    #[test]
    fn cli_resume_requires_id() {
        assert!(Cli::try_parse_from(["ccr", "resume"]).is_err());
    }

    fn sample_session() -> Session {
        Session {
            backend: "claude",
            id: "abc-123".into(),
            cwd: PathBuf::from("/proj"),
            title: "hi".into(),
            last_activity: Local::now(),
            message_count: Some(2),
            preview: Vec::new(),
            possibly_live: false,
            origin: PathBuf::from("<t>"),
            searchable: String::new(),
        }
    }

    fn sample_turns() -> Vec<Turn> {
        vec![
            Turn {
                role: Role::User,
                text: "hello".into(),
            },
            Turn {
                role: Role::Assistant,
                text: "hi back".into(),
            },
        ]
    }

    #[test]
    fn format_md_has_header_and_turns() {
        let md = format_md(&sample_session(), &sample_turns());
        assert!(md.starts_with("# Session `abc-123`"));
        assert!(md.contains("- **Tool:** claude"));
        assert!(md.contains("## ❯ user"));
        assert!(md.contains("hello"));
        assert!(md.contains("## ◆ assistant"));
        assert!(md.contains("hi back"));
    }

    #[test]
    fn format_json_round_trips() {
        let s = sample_session();
        let turns = sample_turns();
        let json = format_json(&s, &turns).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["id"], "abc-123");
        assert_eq!(v["backend"], "claude");
        assert_eq!(v["turns"][0]["role"], "user");
        assert_eq!(v["turns"][1]["role"], "assistant");
    }

    #[test]
    fn format_stats_on_empty_still_prints_zero_row() {
        let out = format_stats(&[]);
        assert!(out.starts_with("Total: 0 sessions"));
    }

    #[test]
    fn fill_counts_replaces_none_with_computed() {
        let mut s = sample_session();
        s.message_count = None;
        let mut sessions = vec![s];
        fill_counts(&mut sessions, |_s| 7);
        assert_eq!(sessions[0].message_count, Some(7));
    }

    #[test]
    fn format_json_message_count_is_turn_count_not_null() {
        let mut s = sample_session();
        s.message_count = None;
        let turns = sample_turns(); // 2 turns
        let json = format_json(&s, &turns).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["message_count"], 2);
        assert!(!v["message_count"].is_null());
    }

    #[test]
    fn format_stats_groups_by_tool_and_project() {
        let mut a = sample_session();
        a.cwd = PathBuf::from("/repos/alpha");
        a.backend = "claude";
        a.message_count = Some(10);
        let mut b = a.clone();
        b.id = "def".into();
        b.backend = "codex";
        b.cwd = PathBuf::from("/repos/beta");
        b.message_count = Some(3);

        let out = format_stats(&[a, b]);
        assert!(out.contains("Total: 2 sessions"));
        assert!(out.contains("13 turns"));
        assert!(out.contains("2 tools"));
        assert!(out.contains("claude"));
        assert!(out.contains("codex"));
        assert!(out.contains("alpha"));
        assert!(out.contains("beta"));
    }
}
