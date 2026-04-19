use anyhow::Result;
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
use std::io;

use crate::backends::{Backend, by_name};
use crate::session::Session;
use crate::util::{project_basename, relative_time, truncate};

pub enum AppAction {
    Resume(Session),
    Quit,
}

enum Mode {
    List,
    Filter,
    Confirm { session: Session, pids: Vec<String> },
    Help,
}

pub fn run(sessions: Vec<Session>, backends: &[Box<dyn Backend>]) -> Result<AppAction> {
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
            Some(sel) if sel >= visible.len() => state.select(if visible.is_empty() {
                None
            } else {
                Some(visible.len() - 1)
            }),
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
        || s.backend.to_lowercase().contains(&needle)
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
            let mut spans = vec![
                Span::styled(
                    format!("[{}] ", s.backend),
                    Style::default().fg(Color::Magenta),
                ),
                Span::styled(
                    format!("{:<18}", truncate(&project, 18)),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {rel}"), Style::default().fg(Color::DarkGray)),
            ];
            if s.possibly_live {
                spans.push(Span::styled(
                    "  ● live",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            let title = Line::from(Span::styled(
                format!("  {}", s.title),
                Style::default().fg(Color::White),
            ));
            ListItem::new(vec![Line::from(spans), title])
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

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("tool:   ", Style::default().fg(Color::DarkGray)),
            Span::styled(s.backend, Style::default().fg(Color::Magenta)),
        ]),
        Line::from(vec![
            Span::styled("cwd:    ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                s.cwd.display().to_string(),
                Style::default().fg(Color::Green),
            ),
        ]),
        Line::from(vec![
            Span::styled("last:   ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!(
                "{}  ({})",
                s.last_activity.format("%Y-%m-%d %H:%M"),
                relative_time(s.last_activity)
            )),
        ]),
        Line::from(vec![
            Span::styled("msgs:   ", Style::default().fg(Color::DarkGray)),
            Span::raw(s.message_count.to_string()),
        ]),
        Line::from(vec![
            Span::styled("id:     ", Style::default().fg(Color::DarkGray)),
            Span::raw(s.id.clone()),
        ]),
    ];
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
        Line::from(vec![
            Span::styled("tool:    ", Style::default().fg(Color::DarkGray)),
            Span::raw(session.backend),
        ]),
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
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        area,
    );
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
        k("/", "filter by title, cwd, or tool"),
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
