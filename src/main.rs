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
    /// Restore a previously soft-deleted session from ~/.ccr/trash/.
    Restore {
        /// Session id to restore. Omit for interactive numeric prompt.
        id: Option<String>,
    },
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
    let _ = trash::auto_prune();

    match cli.command {
        None => launch_picker(),
        Some(Command::Prune {
            older_than,
            dry_run,
        }) => run_prune(&older_than, dry_run),
        Some(Command::List) => run_list(),
        Some(Command::Restore { id }) => run_restore(id.as_deref()),
        Some(Command::Path { id }) => run_path(&id),
        Some(Command::Show { id }) => run_show(&id),
        Some(Command::Export { id, format }) => run_export(&id, format),
        Some(Command::Stats) => run_stats(),
    }
}

fn find_session_by_id(id: &str) -> Result<Session> {
    let backends = all();
    scan_all(&backends)
        .into_iter()
        .find(|s| s.id == id)
        .with_context(|| format!("no session with id `{id}`"))
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

fn run_export(id: &str, format: ExportFormat) -> Result<()> {
    let backends = all();
    let session = scan_all(&backends)
        .into_iter()
        .find(|s| s.id == id)
        .with_context(|| format!("no session with id `{id}`"))?;
    let backend = by_name(&backends, session.backend)
        .with_context(|| format!("unknown backend `{}`", session.backend))?;
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

fn run_stats() -> Result<()> {
    let backends = all();
    let sessions = scan_all(&backends);
    print!("{}", format_stats(&sessions));
    Ok(())
}

pub(crate) fn format_stats(sessions: &[Session]) -> String {
    use std::collections::BTreeMap;
    use std::collections::HashMap;

    let mut out = String::new();
    let total = sessions.len();
    let total_turns: usize = sessions.iter().map(|s| s.message_count).sum();
    let tools: std::collections::BTreeSet<&str> = sessions.iter().map(|s| s.backend).collect();

    out.push_str(&format!(
        "Total: {total} session{}  ·  {total_turns} turn{}  ·  {} tool{}\n",
        plural(total),
        plural(total_turns),
        tools.len(),
        plural(tools.len()),
    ));

    // Per-tool
    let mut by_tool: HashMap<&str, (usize, usize)> = HashMap::new();
    for s in sessions {
        let e = by_tool.entry(s.backend).or_default();
        e.0 += 1;
        e.1 += s.message_count;
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

    // Per-project (top 10)
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

    // Last 30 days histogram
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
            let width = (count * 30 + max / 2) / max; // rounded
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
        "message_count": s.message_count,
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

fn run_restore(id: Option<&str>) -> Result<()> {
    let items = trash::list_trashed()?;
    if items.is_empty() {
        println!("ccr: trash is empty");
        return Ok(());
    }

    if let Some(needle) = id {
        let item = items
            .iter()
            .find(|i| i.id == needle)
            .with_context(|| format!("no trashed session with id `{needle}`"))?;
        trash::restore(item)?;
        println!(
            "restored [{}] {} → {}",
            item.backend,
            item.id,
            item.origin.display()
        );
        return Ok(());
    }

    println!("Trashed sessions:\n");
    for (i, item) in items.iter().enumerate() {
        let age = chrono::DateTime::<chrono::Local>::from(item.trashed_at).format("%Y-%m-%d %H:%M");
        println!(
            "  {:>3}. [{}] {}  trashed {}  → {}",
            i + 1,
            item.backend,
            item.id,
            age,
            item.origin.display()
        );
    }
    print!("\nRestore # (q to cancel): ");
    use std::io::Write;
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let line = line.trim();
    if line.is_empty() || line == "q" {
        println!("cancelled");
        return Ok(());
    }
    let n: usize = line
        .parse()
        .with_context(|| format!("invalid number: {line}"))?;
    let item = items
        .get(n.saturating_sub(1))
        .with_context(|| format!("out of range: {n}"))?;
    trash::restore(item)?;
    println!(
        "restored [{}] {} → {}",
        item.backend,
        item.id,
        item.origin.display()
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;
    use std::path::PathBuf;

    fn sample_session() -> Session {
        Session {
            backend: "claude",
            id: "abc-123".into(),
            cwd: PathBuf::from("/proj"),
            title: "hi".into(),
            last_activity: Local::now(),
            message_count: 2,
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
    fn format_stats_groups_by_tool_and_project() {
        let mut a = sample_session();
        a.cwd = PathBuf::from("/repos/alpha");
        a.backend = "claude";
        a.message_count = 10;
        let mut b = a.clone();
        b.id = "def".into();
        b.backend = "codex";
        b.cwd = PathBuf::from("/repos/beta");
        b.message_count = 3;

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
