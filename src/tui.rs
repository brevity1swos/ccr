use anyhow::Result;
use crossterm::{
    cursor::Show as ShowCursor,
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
use std::collections::{HashMap, HashSet};
use std::io;

use crate::backends::{Backend, by_name};
use crate::bookmarks;
use crate::nicknames;
use crate::session::{Role, Session};
use crate::util::{project_basename, relative_time, truncate};

const PAGE_JUMP: i32 = 10;
const PROJECT_COL_WIDTH: usize = 18;
const PREVIEW_LINE_WIDTH: usize = 120;
const PREVIEW_LINES_PER_TURN: usize = 8;

fn dim<S: Into<std::borrow::Cow<'static, str>>>(s: S) -> Span<'static> {
    Span::styled(s, Style::default().fg(Color::DarkGray))
}

pub enum AppAction {
    Resume(Session),
    View(Session),
    Quit,
}

enum Mode {
    List,
    Filter,
    Confirm { session: Session, pids: Vec<String> },
    Help,
    NicknameInput { session_id: String, buf: String },
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            ShowCursor
        );
    }
}

pub fn run(sessions: Vec<Session>, backends: &[Box<dyn Backend>]) -> Result<AppAction> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let _guard = TerminalGuard;

    let mut state = ListState::default();
    if !sessions.is_empty() {
        state.select(Some(0));
    }
    let mut filter = String::new();
    let mut mode = Mode::List;
    let mut bookmarked: HashSet<String> = bookmarks::load();
    let mut nicknames: HashMap<String, String> = nicknames::load();

    let result = loop {
        let visible: Vec<&Session> = sessions
            .iter()
            .filter(|s| matches_filter(s, &filter, &nicknames))
            .collect();
        match state.selected() {
            Some(sel) if sel >= visible.len() => state.select(if visible.is_empty() {
                None
            } else {
                Some(visible.len() - 1)
            }),
            None if !visible.is_empty() => state.select(Some(0)),
            _ => {}
        }

        terminal.draw(|f| {
            ui(
                f,
                &visible,
                &mut state,
                &filter,
                &mode,
                &bookmarked,
                &nicknames,
            )
        })?;

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
                if matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Char('?') | KeyCode::F(1) | KeyCode::Char('q')
                ) {
                    mode = Mode::List;
                }
            }
            Mode::Confirm { session, .. } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    break AppAction::Resume(session.clone());
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => mode = Mode::List,
                _ => {}
            },
            Mode::NicknameInput { session_id, buf } => {
                let id = session_id.clone();
                let mut next_buf = buf.clone();
                match key.code {
                    KeyCode::Esc => mode = Mode::List,
                    KeyCode::Enter => {
                        let _ = nicknames::set(&mut nicknames, &id, &next_buf);
                        mode = Mode::List;
                    }
                    KeyCode::Backspace => {
                        next_buf.pop();
                        mode = Mode::NicknameInput {
                            session_id: id,
                            buf: next_buf,
                        };
                    }
                    KeyCode::Char(c) => {
                        next_buf.push(c);
                        mode = Mode::NicknameInput {
                            session_id: id,
                            buf: next_buf,
                        };
                    }
                    _ => {}
                }
            }
            Mode::List => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break AppAction::Quit,
                KeyCode::Char('/') => {
                    mode = Mode::Filter;
                    filter.clear();
                }
                KeyCode::Char('?') | KeyCode::F(1) => mode = Mode::Help,
                KeyCode::Down | KeyCode::Char('j') => move_sel(&mut state, &visible, 1),
                KeyCode::Up | KeyCode::Char('k') => move_sel(&mut state, &visible, -1),
                KeyCode::PageDown => move_sel(&mut state, &visible, PAGE_JUMP),
                KeyCode::PageUp => move_sel(&mut state, &visible, -PAGE_JUMP),
                KeyCode::Home | KeyCode::Char('g') if !visible.is_empty() => {
                    state.select(Some(0));
                }
                KeyCode::End | KeyCode::Char('G') if !visible.is_empty() => {
                    state.select(Some(visible.len() - 1));
                }
                KeyCode::Char('b') | KeyCode::Char('B') => {
                    if let Some(sel) = state.selected()
                        && let Some(s) = visible.get(sel)
                    {
                        let _ = bookmarks::toggle(&mut bookmarked, &s.id);
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    if let Some(sel) = state.selected()
                        && let Some(s) = visible.get(sel)
                    {
                        let existing = nicknames.get(&s.id).cloned().unwrap_or_default();
                        mode = Mode::NicknameInput {
                            session_id: s.id.clone(),
                            buf: existing,
                        };
                    }
                }
                KeyCode::Char('v') | KeyCode::Char('V') => {
                    if let Some(sel) = state.selected()
                        && let Some(s) = visible.get(sel)
                    {
                        break AppAction::View((*s).clone());
                    }
                }
                KeyCode::Enter => {
                    if let Some(sel) = state.selected()
                        && let Some(s) = visible.get(sel)
                    {
                        let pids = by_name(backends, s.backend)
                            .map(|b| b.running(s))
                            .unwrap_or_default();
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

    Ok(result)
}

fn matches_filter(s: &Session, filter: &str, nicknames: &HashMap<String, String>) -> bool {
    if filter.is_empty() {
        return true;
    }
    let needle = filter.to_lowercase();
    s.title.to_lowercase().contains(&needle)
        || s.cwd.to_string_lossy().to_lowercase().contains(&needle)
        || s.backend.to_lowercase().contains(&needle)
        || s.searchable.contains(&needle)
        || nicknames
            .get(&s.id)
            .is_some_and(|n| n.to_lowercase().contains(&needle))
}

fn move_sel(state: &mut ListState, visible: &[&Session], delta: i32) {
    if visible.is_empty() {
        return;
    }
    let cur = state.selected().unwrap_or(0) as i32;
    let new = (cur + delta).clamp(0, visible.len() as i32 - 1);
    state.select(Some(new as usize));
}

fn ui(
    f: &mut Frame,
    sessions: &[&Session],
    state: &mut ListState,
    filter: &str,
    mode: &Mode,
    bookmarked: &HashSet<String>,
    nicknames: &HashMap<String, String>,
) {
    let size = f.area();
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(size);

    let live = sessions.iter().filter(|s| s.possibly_live).count();
    let header = format!(
        " ccr — {} session{}  {}{}",
        sessions.len(),
        if sessions.len() == 1 { "" } else { "s" },
        if live > 0 {
            format!("({live} possibly live)  ")
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
        Paragraph::new(header).style(
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

    render_list(f, columns[0], sessions, state, bookmarked, nicknames);
    render_preview(f, columns[1], sessions, state, nicknames);

    let footer = match mode {
        Mode::Filter => Span::styled(
            format!(" / {filter}_  (Enter to apply, Esc to cancel) "),
            Style::default().fg(Color::Yellow),
        ),
        Mode::Confirm { .. } => Span::styled(
            " confirm: y = resume anyway · n/Esc = cancel ",
            Style::default().fg(Color::Yellow),
        ),
        Mode::NicknameInput { .. } => Span::styled(
            " n: set nickname  (Enter save · Esc cancel · empty = remove) ",
            Style::default().fg(Color::Yellow),
        ),
        Mode::Help => Span::styled(" ? / Esc to close ", Style::default().fg(Color::DarkGray)),
        Mode::List => Span::styled(
            " Enter resume · v view · b bookmark · n nickname · / filter · ? help · q quit ",
            Style::default().fg(Color::DarkGray),
        ),
    };
    f.render_widget(Paragraph::new(Line::from(footer)), outer[2]);

    match mode {
        Mode::Confirm { session, pids } => render_confirm(f, size, session, pids),
        Mode::Help => render_help(f, size),
        Mode::NicknameInput { session_id, buf } => render_nickname_input(f, size, session_id, buf),
        Mode::List | Mode::Filter => {}
    }
}

fn render_list(
    f: &mut Frame,
    area: Rect,
    sessions: &[&Session],
    state: &mut ListState,
    bookmarked: &HashSet<String>,
    nicknames: &HashMap<String, String>,
) {
    let items: Vec<ListItem> = sessions
        .iter()
        .map(|s| {
            let project = project_basename(&s.cwd);
            let rel = relative_time(s.last_activity);
            let mut spans = vec![Span::styled(
                if bookmarked.contains(&s.id) {
                    "★ "
                } else {
                    "  "
                },
                Style::default().fg(Color::Yellow),
            )];
            spans.extend([
                Span::styled(
                    format!("[{}] ", s.backend),
                    Style::default().fg(Color::Magenta),
                ),
                Span::styled(
                    format!(
                        "{:<w$}",
                        truncate(&project, PROJECT_COL_WIDTH),
                        w = PROJECT_COL_WIDTH
                    ),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {rel}"), Style::default().fg(Color::DarkGray)),
            ]);
            if s.possibly_live {
                spans.push(Span::styled(
                    "  ● live",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            // Nicknamed rows are 3 lines: tags, yellow nickname, then the
            // auto-derived last-message title (dim) for context.
            // Unnicknamed rows are 2 lines: tags + title (white).
            let mut lines = vec![Line::from(spans)];
            if let Some(nick) = nicknames.get(&s.id) {
                lines.push(Line::from(Span::styled(
                    format!("  {nick}"),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(Span::styled(
                    format!("  {}", s.title),
                    Style::default().fg(Color::DarkGray),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    format!("  {}", s.title),
                    Style::default().fg(Color::White),
                )));
            }
            ListItem::new(lines)
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

fn render_preview(
    f: &mut Frame,
    area: Rect,
    sessions: &[&Session],
    state: &ListState,
    nicknames: &HashMap<String, String>,
) {
    let block = Block::default().borders(Borders::ALL).title(" Preview ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(sel) = state.selected() else { return };
    let Some(s) = sessions.get(sel) else { return };

    let mut lines: Vec<Line> = Vec::new();

    if let Some(nick) = nicknames.get(&s.id) {
        lines.push(Line::from(vec![
            dim("nick:   "),
            Span::styled(
                nick.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    lines.extend([
        Line::from(vec![
            dim("tool:   "),
            Span::styled(s.backend, Style::default().fg(Color::Magenta)),
        ]),
        Line::from(vec![
            dim("cwd:    "),
            Span::styled(
                s.cwd.display().to_string(),
                Style::default().fg(Color::Green),
            ),
        ]),
        Line::from(vec![
            dim("last:   "),
            Span::raw(format!(
                "{}  ({})",
                s.last_activity.format("%Y-%m-%d %H:%M"),
                relative_time(s.last_activity)
            )),
        ]),
        Line::from(vec![
            dim("msgs:   "),
            Span::raw(
                s.message_count
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "…".into()),
            ),
        ]),
        Line::from(vec![dim("id:     "), Span::raw(s.id.clone())]),
    ]);

    if s.possibly_live {
        lines.push(Line::from(Span::styled(
            "status: ● recently active — may be running",
            Style::default().fg(Color::Yellow),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(dim("── recent turns ──")));

    for t in &s.preview {
        let (tag, color) = match t.role {
            Role::User => ("❯ user", Color::Cyan),
            Role::Assistant => ("◆ asst", Color::Magenta),
        };
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            tag,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )));
        for raw in t.text.lines().take(PREVIEW_LINES_PER_TURN) {
            lines.push(Line::from(Span::raw(truncate(raw, PREVIEW_LINE_WIDTH))));
        }
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

fn render_confirm(f: &mut Frame, area: Rect, session: &Session, pids: &[String]) {
    let area = centered(area, 80, (pids.len() as u16 + 11).min(20));
    f.render_widget(Clear, area);

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "⚠  Session may already be running",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![dim("tool:    "), Span::raw(session.backend)]),
        Line::from(vec![dim("session: "), Span::raw(&session.id)]),
        Line::from(vec![
            dim("cwd:     "),
            Span::raw(session.cwd.display().to_string()),
        ]),
        Line::from(""),
        Line::from(dim("matching processes:")),
    ];
    for p in pids {
        lines.push(Line::from(Span::raw(truncate(p, 76))));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Resuming may interleave writes and corrupt the session.",
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
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_nickname_input(f: &mut Frame, area: Rect, session_id: &str, buf: &str) {
    let area = centered(area, 62, 8);
    f.render_widget(Clear, area);
    let lines = vec![
        Line::from(Span::styled(
            "Set session nickname:",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            dim("  id:    "),
            Span::styled(
                truncate(session_id, 44),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(vec![
            dim("  name:  "),
            Span::styled(
                format!("{buf}_"),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Enter save · Esc cancel · (empty = remove nickname)",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Nickname ")
        .border_style(Style::default().fg(Color::Yellow));
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_help(f: &mut Frame, area: Rect) {
    let area = centered(area, 70, 20);
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
        k("v", "view session turns in agx (requires `agx` on PATH)"),
        k(
            "b",
            "toggle bookmark (★ marker, persists in ~/.ccr/bookmarks.json)",
        ),
        k(
            "n",
            "set nickname — shown in yellow instead of last message",
        ),
        k("/", "filter: title, cwd, tool, nickname, or content"),
        k("? / F1", "this help"),
        k("q / Esc", "quit"),
        Line::from(""),
        section("Indicators"),
        k("[tool]", "which CLI assistant owns the session"),
        k("● live", "modified < 5 min ago — may be running"),
        Line::from(""),
        Line::from(Span::styled(
            "On Enter, ccr runs `pgrep -f <id>` and prompts if a",
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;
    use std::path::PathBuf;

    fn sess(title: &str, cwd: &str, backend: &'static str) -> Session {
        Session {
            backend,
            id: "x".into(),
            cwd: PathBuf::from(cwd),
            title: title.into(),
            last_activity: Local::now(),
            message_count: Some(0),
            preview: Vec::new(),
            possibly_live: false,
            origin: PathBuf::from("<test>"),
            searchable: String::new(),
        }
    }

    fn no_nicknames() -> HashMap<String, String> {
        HashMap::new()
    }

    #[test]
    fn empty_filter_matches_everything() {
        assert!(matches_filter(
            &sess("hello", "/x", "claude"),
            "",
            &no_nicknames()
        ));
    }

    #[test]
    fn filter_matches_title_case_insensitive() {
        assert!(matches_filter(
            &sess("Hello World", "/x", "claude"),
            "HELLO",
            &no_nicknames()
        ));
        assert!(matches_filter(
            &sess("hello world", "/x", "claude"),
            "Hello",
            &no_nicknames()
        ));
    }

    #[test]
    fn filter_matches_cwd() {
        assert!(matches_filter(
            &sess("x", "/home/me/proj", "claude"),
            "proj",
            &no_nicknames()
        ));
    }

    #[test]
    fn filter_matches_backend_tag() {
        assert!(matches_filter(
            &sess("x", "/y", "claude"),
            "claud",
            &no_nicknames()
        ));
    }

    #[test]
    fn filter_rejects_no_match() {
        assert!(!matches_filter(
            &sess("hello", "/y", "claude"),
            "xyz",
            &no_nicknames()
        ));
    }

    #[test]
    fn filter_matches_full_turn_content() {
        let mut s = sess("unrelated title", "/x", "claude");
        s.searchable = "the panic came from a race on ccr_trash_dir".into();
        assert!(matches_filter(&s, "race", &no_nicknames()));
        assert!(matches_filter(&s, "CCR_TRASH_DIR", &no_nicknames()));
        assert!(!matches_filter(&s, "nonexistentword", &no_nicknames()));
    }

    #[test]
    fn filter_matches_nickname() {
        let s = sess("last user message", "/x", "claude");
        let mut nicks = HashMap::new();
        nicks.insert("x".into(), "auth refactor sprint".into());
        assert!(matches_filter(&s, "auth", &nicks));
        assert!(matches_filter(&s, "SPRINT", &nicks)); // case-insensitive
        assert!(!matches_filter(&s, "xyz", &nicks));
    }

    #[test]
    fn move_sel_clamps_to_bounds() {
        let mut state = ListState::default();
        let s = sess("x", "/y", "claude");
        let refs: Vec<&Session> = vec![&s, &s, &s];
        move_sel(&mut state, &refs, 2);
        assert_eq!(state.selected(), Some(2));
        move_sel(&mut state, &refs, 5);
        assert_eq!(state.selected(), Some(2));
        move_sel(&mut state, &refs, -10);
        assert_eq!(state.selected(), Some(0));
    }

    #[test]
    fn move_sel_is_noop_on_empty_list() {
        let mut state = ListState::default();
        let empty: Vec<&Session> = Vec::new();
        move_sel(&mut state, &empty, 1);
        assert_eq!(state.selected(), None);
    }
}
