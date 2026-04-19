use anyhow::{Context, Result};
use chrono::{DateTime, Local, TimeZone};
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::backends::Backend;
use crate::session::{PREVIEW_TURNS, Session, TITLE_MAX, Turn};
use crate::util::{is_possibly_live, truncate};

pub struct ClaudeBackend;

impl ClaudeBackend {
    const NAME: &'static str = "claude";

    /// Resolve the Claude Code `projects/` directory.
    ///
    /// Precedence:
    /// 1. `CCR_CLAUDE_DIR` — full path to the `projects/` dir (escape hatch)
    /// 2. `CLAUDE_CONFIG_DIR` — Claude Code's own override; we append `projects`
    /// 3. `~/.claude/projects` — default
    fn projects_dir() -> Result<PathBuf> {
        if let Ok(dir) = std::env::var("CCR_CLAUDE_DIR") {
            return Ok(PathBuf::from(dir));
        }
        if let Ok(config) = std::env::var("CLAUDE_CONFIG_DIR") {
            return Ok(PathBuf::from(config).join("projects"));
        }
        let home = dirs::home_dir().context("no home dir")?;
        Ok(home.join(".claude").join("projects"))
    }
}

impl Backend for ClaudeBackend {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn scan(&self) -> Result<Vec<Session>> {
        let root = Self::projects_dir()?;
        if !root.exists() {
            return Ok(Vec::new());
        }
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
        Ok(out)
    }

    fn resume(&self, s: &Session) -> Command {
        let mut cmd = Command::new("claude");
        cmd.arg("--resume").arg(&s.id).current_dir(&s.cwd);
        cmd
    }
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

pub(crate) fn parse_session(path: &Path) -> Result<Option<Session>> {
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
    let possibly_live = is_possibly_live(last_activity);

    let preview_start = turns.len().saturating_sub(PREVIEW_TURNS);
    let preview = turns[preview_start..].to_vec();

    Ok(Some(Session {
        backend: ClaudeBackend::NAME,
        id,
        cwd,
        title,
        last_activity,
        message_count,
        preview,
        possibly_live,
    }))
}
