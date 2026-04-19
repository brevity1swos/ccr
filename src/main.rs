use anyhow::{Context, Result};
use chrono::{DateTime, Local, TimeZone};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::CrosstermBackend,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use serde_json::Value;
use std::{
    fs,
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
    process::Command,
};

const TITLE_MAX: usize = 80;
const PREVIEW_TURNS: usize = 6;
const LIVE_WINDOW_SECS: i64 = 300;

#[derive(Debug, Clone)]
struct Turn {
    role: String,
    text: String,
}

#[derive(Debug, Clone)]
struct Session {
    id: String,
    cwd: PathBuf,
    title: String,
    last_activity: DateTime<Local>,
    message_count: usize,
    preview: Vec<Turn>,
    possibly_live: bool,
}

fn claude_projects_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("no home dir")?;
    Ok(home.join(".claude").join("projects"))
}

fn scan_sessions() -> Result<Vec<Session>> {
    let root = claude_projects_dir()?;
    let mut out = Vec::new();
    for entry in fs::read_dir(&root).with_context(|| format!("read_dir {}", root.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        for f in fs::read_dir(entry.path())? {
            let f = f?;
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Ok(Some(s)) = parse_session(&p) {
                out.push(s);
            }
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.last_activity));
    Ok(out)
}

fn extract_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr
            .iter()
            .filter_map(|c| {
                if c.get("type").and_then(|t| t.as_str()) == Some("text") {
                    c.get("text").and_then(|t| t.as_str()).map(String::from)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn parse_session(path: &Path) -> Result<Option<Session>> {
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    if id.is_empty() {
        return Ok(None);
    }

    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);

    let mut cwd: Option<PathBuf> = None;
    let mut title: Option<String> = None;
    let mut last_ts: Option<DateTime<Local>> = None;
    let mut message_count = 0usize;
    let mut turns: Vec<Turn> = Vec::new();

    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        if let Some(c) = v.get("cwd").and_then(|c| c.as_str())
            && cwd.is_none()
        {
            cwd = Some(PathBuf::from(c));
        }
        if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str())
            && let Ok(parsed) = DateTime::parse_from_rfc3339(ts)
        {
            last_ts = Some(parsed.with_timezone(&Local));
        }

        let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if kind == "user" || kind == "assistant" {
            let content = v
                .get("message")
                .and_then(|m| m.get("content"))
                .cloned()
                .unwrap_or(Value::Null);
            let text = extract_text(&content);
            if text.trim().is_empty() {
                continue;
            }
            message_count += 1;
            if kind == "user" && title.is_none() {
                title = Some(truncate(&text, TITLE_MAX));
            }
            turns.push(Turn {
                role: kind.to_string(),
                text,
            });
        }
    }

    let cwd = cwd.unwrap_or_else(|| PathBuf::from("(unknown)"));
    let title = title.unwrap_or_else(|| "(no user message)".into());
    let last_activity = last_ts.unwrap_or_else(|| Local.timestamp_opt(0, 0).unwrap());
    let possibly_live =
        Local::now().signed_duration_since(last_activity).num_seconds() < LIVE_WINDOW_SECS;

    let preview_start = turns.len().saturating_sub(PREVIEW_TURNS);
    let preview = turns[preview_start..].to_vec();

    Ok(Some(Session {
        id,
        cwd,
        title,
        last_activity,
        message_count,
        preview,
        possibly_live,
    }))
}

fn truncate(s: &str, max: usize) -> String {
    let cleaned = s.replace(['\n', '\r'], " ");
    if cleaned.chars().count() <= max {
        cleaned
    } else {
        let mut out: String = cleaned.chars().take(max).collect();
        out.push('…');
        out
    }
}

fn relative_time(dt: DateTime<Local>) -> String {
    let now = Local::now();
    let delta = now.signed_duration_since(dt);
    let s = delta.num_seconds();
    if s < 60 {
        format!("{s}s ago")
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else if s < 86400 {
        format!("{}h ago", s / 3600)
    } else if s < 86400 * 30 {
        format!("{}d ago", s / 86400)
    } else {
        dt.format("%Y-%m-%d").to_string()
    }
}

fn project_basename(p: &Path) -> String {
    p.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_string()
}

fn find_running(uuid: &str) -> Vec<String> {
    let own_pid = std::process::id().to_string();
    let Ok(out) = Command::new("pgrep").args(["-af", uuid]).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| {
            let pid = l.split_whitespace().next().unwrap_or("");
            pid != own_pid && !l.contains(" ccr") && !l.ends_with("/ccr")
        })
        .map(String::from)
        .collect()
}

enum AppAction {
    Resume(Session),
    Quit,
}

enum Mode {
    List,
    Filter,
    Confirm { session: Session, pids: Vec<String> },
    Help,
}

fn run_tui(sessions: Vec<Session>) -> Result<AppAction> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = ListState::default();
    if !sessions.is_empty() {
        state.select(Some(0));
    }
    let mut filter = String::new();
    let mut mode = Mode::List;

    let result = loop {
        let visible: Vec<&Session> = sessions
            .iter()
            .filter(|s| matches_filter(s, &filter))
            .collect();
        match state.selected() {
            Some(sel) if sel >= visible.len() => {
                state.select(if visible.is_empty() {
                    None
                } else {
                    Some(visible.len() - 1)
                });
            }
            None if !visible.is_empty() => state.select(Some(0)),
            _ => {}
        }

        terminal.draw(|f| ui(f, &visible, &mut state, &filter, &mode))?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match &mode {
            Mode::Filter => match key.code {
                KeyCode::Esc => {
                    mode = Mode::List;
                    filter.clear();
                }
                KeyCode::Enter => mode = Mode::List,
                KeyCode::Backspace => {
                    filter.pop();
                }
                KeyCode::Char(c) => filter.push(c),
                _ => {}
            },
            Mode::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('?') | KeyCode::F(1) | KeyCode::Char('q'))
                {
                    mode = Mode::List;
                }
            }
            Mode::Confirm { session, .. } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    break AppAction::Resume(session.clone());
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    mode = Mode::List;
                }
                _ => {}
            },
            Mode::List => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break AppAction::Quit,
                KeyCode::Char('/') => {
                    mode = Mode::Filter;
                    filter.clear();
                }
                KeyCode::Char('?') | KeyCode::F(1) => mode = Mode::Help,
                KeyCode::Down | KeyCode::Char('j') => move_sel(&mut state, &visible, 1),
                KeyCode::Up | KeyCode::Char('k') => move_sel(&mut state, &visible, -1),
                KeyCode::PageDown => move_sel(&mut state, &visible, 10),
                KeyCode::PageUp => move_sel(&mut state, &visible, -10),
                KeyCode::Home | KeyCode::Char('g') => {
                    if !visible.is_empty() {
                        state.select(Some(0));
                    }
                }
                KeyCode::End | KeyCode::Char('G') => {
                    if !visible.is_empty() {
                        state.select(Some(visible.len() - 1));
                    }
                }
                KeyCode::Enter => {
                    if let Some(sel) = state.selected()
                        && let Some(s) = visible.get(sel)
                    {
                        let pids = find_running(&s.id);
                        if pids.is_empty() {
                            break AppAction::Resume((*s).clone());
                        }
                        mode = Mode::Confirm {
                            session: (*s).clone(),
                            pids,
                        };
                    }
                }
                _ => {}
            },
        }
    };

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(result)
}

fn matches_filter(s: &Session, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let needle = filter.to_lowercase();
    s.title.to_lowercase().contains(&needle)
        || s.cwd.to_string_lossy().to_lowercase().contains(&needle)
}

fn move_sel(state: &mut ListState, visible: &[&Session], delta: i32) {
    if visible.is_empty() {
        return;
    }
    let cur = state.selected().unwrap_or(0) as i32;
    let new = (cur + delta).clamp(0, visible.len() as i32 - 1);
    state.select(Some(new as usize));
}

fn ui(f: &mut Frame, sessions: &[&Session], state: &mut ListState, filter: &str, mode: &Mode) {
    let size = f.area();
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(size);

    let live_count = sessions.iter().filter(|s| s.possibly_live).count();
    let header_text = format!(
        " ccr — {} session{}  {}{}",
        sessions.len(),
        if sessions.len() == 1 { "" } else { "s" },
        if live_count > 0 {
            format!("({live_count} possibly live)  ")
        } else {
            String::new()
        },
        if filter.is_empty() {
            String::new()
        } else {
            format!("[filter: {filter}]")
        }
    );
    f.render_widget(
        Paragraph::new(header_text).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        outer[0],
    );

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(outer[1]);

    render_list(f, columns[0], sessions, state);
    render_preview(f, columns[1], sessions, state);

    let footer = match mode {
        Mode::Filter => Span::styled(
            format!(" / {filter}_  (Enter to apply, Esc to cancel) "),
            Style::default().fg(Color::Yellow),
        ),
        Mode::Confirm { .. } => Span::styled(
            " confirm: y = resume anyway · n/Esc = cancel ",
            Style::default().fg(Color::Yellow),
        ),
        Mode::Help => Span::styled(" ? / Esc to close ", Style::default().fg(Color::DarkGray)),
        Mode::List => Span::styled(
            " ↑↓/jk · g/G top/bottom · Enter resume · / filter · ? help · q quit ",
            Style::default().fg(Color::DarkGray),
        ),
    };
    f.render_widget(Paragraph::new(Line::from(footer)), outer[2]);

    match mode {
        Mode::Confirm { session, pids } => render_confirm(f, size, session, pids),
        Mode::Help => render_help(f, size),
        _ => {}
    }
}

fn render_list(f: &mut Frame, area: Rect, sessions: &[&Session], state: &mut ListState) {
    let items: Vec<ListItem> = sessions
        .iter()
        .map(|s| {
            let project = project_basename(&s.cwd);
            let rel = relative_time(s.last_activity);
            let mut header_spans = vec![
                Span::styled(
                    format!("{:<20}", truncate(&project, 20)),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {rel}"), Style::default().fg(Color::DarkGray)),
            ];
            if s.possibly_live {
                header_spans.push(Span::styled(
                    "  ● live",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            let header = Line::from(header_spans);
            let title = Line::from(Span::styled(
                format!("  {}", s.title),
                Style::default().fg(Color::White),
            ));
            ListItem::new(vec![header, title])
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Sessions "))
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, area, state);
}

fn render_preview(f: &mut Frame, area: Rect, sessions: &[&Session], state: &mut ListState) {
    let block = Block::default().borders(Borders::ALL).title(" Preview ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(sel) = state.selected() else { return };
    let Some(s) = sessions.get(sel) else { return };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("cwd:    ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            s.cwd.display().to_string(),
            Style::default().fg(Color::Green),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("last:   ", Style::default().fg(Color::DarkGray)),
        Span::raw(format!(
            "{}  ({})",
            s.last_activity.format("%Y-%m-%d %H:%M"),
            relative_time(s.last_activity)
        )),
    ]));
    lines.push(Line::from(vec![
        Span::styled("msgs:   ", Style::default().fg(Color::DarkGray)),
        Span::raw(s.message_count.to_string()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("id:     ", Style::default().fg(Color::DarkGray)),
        Span::raw(s.id.clone()),
    ]));
    if s.possibly_live {
        lines.push(Line::from(Span::styled(
            "status: ● recently active — may be running",
            Style::default().fg(Color::Yellow),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "── recent turns ──",
        Style::default().fg(Color::DarkGray),
    )));

    for t in &s.preview {
        let (tag, color) = if t.role == "user" {
            ("❯ user", Color::Cyan)
        } else {
            ("◆ asst", Color::Magenta)
        };
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            tag,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )));
        for raw in t.text.lines().take(8) {
            lines.push(Line::from(Span::raw(truncate(raw, 120))));
        }
    }

    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    f.render_widget(para, inner);
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

fn render_confirm(f: &mut Frame, area: Rect, session: &Session, pids: &[String]) {
    let area = centered(area, 80, (pids.len() as u16 + 10).min(20));
    f.render_widget(Clear, area);

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "⚠  Session may already be running",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("session: ", Style::default().fg(Color::DarkGray)),
            Span::raw(&session.id),
        ]),
        Line::from(vec![
            Span::styled("cwd:     ", Style::default().fg(Color::DarkGray)),
            Span::raw(session.cwd.display().to_string()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "matching processes:",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    for p in pids {
        lines.push(Line::from(Span::raw(truncate(p, 76))));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Resuming may interleave JSONL writes and corrupt the session.",
        Style::default().fg(Color::Red),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "[y]",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" resume anyway    "),
        Span::styled(
            "[n]",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" cancel"),
    ]));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Confirm resume ")
        .border_style(Style::default().fg(Color::Yellow));
    let para = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    f.render_widget(para, area);
}

fn render_help(f: &mut Frame, area: Rect) {
    let area = centered(area, 70, 22);
    f.render_widget(Clear, area);

    let k = |key: &'static str, desc: &'static str| -> Line<'static> {
        Line::from(vec![
            Span::styled(
                format!("  {key:<12}"),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(desc),
        ])
    };
    let section = |name: &'static str| -> Line<'static> {
        Line::from(Span::styled(
            name,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ))
    };

    let lines = vec![
        section("Navigation"),
        k("↑ / k", "up"),
        k("↓ / j", "down"),
        k("g / Home", "jump to top"),
        k("G / End", "jump to bottom"),
        k("PgUp / PgDn", "page up / down (10 rows)"),
        Line::from(""),
        section("Actions"),
        k("Enter", "resume selected session (with live-check)"),
        k("/", "filter by title or cwd"),
        k("? / F1", "this help"),
        k("q / Esc", "quit"),
        Line::from(""),
        section("Indicators"),
        k("● live", "modified < 5 min ago — may be running"),
        Line::from(""),
        Line::from(Span::styled(
            "On Enter, ccr runs `pgrep -f <uuid>` and prompts if a claude",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "process is already attached to the session.",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Help — ccr ")
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn main() -> Result<()> {
    let sessions = scan_sessions()?;
    if sessions.is_empty() {
        eprintln!("No Claude Code sessions found in ~/.claude/projects/");
        std::process::exit(1);
    }
    match run_tui(sessions)? {
        AppAction::Quit => Ok(()),
        AppAction::Resume(s) => {
            let status = Command::new("claude")
                .arg("--resume")
                .arg(&s.id)
                .current_dir(&s.cwd)
                .status()
                .context("failed to spawn `claude` — is the CLI on PATH?")?;
            std::process::exit(status.code().unwrap_or(1));
        }
    }
}
