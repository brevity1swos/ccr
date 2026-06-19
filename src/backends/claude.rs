use anyhow::{Context, Result};
use chrono::{DateTime, Local, TimeZone};
use serde_json::Value;
use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::Command;

use rayon::prelude::*;

use crate::backends::Backend;
use crate::session::{PREVIEW_TURNS, Role, Session, TITLE_MAX, Turn, append_searchable};
use crate::util::{is_possibly_live, truncate};

pub struct ClaudeBackend;

impl ClaudeBackend {
    const NAME: &'static str = "claude";

    /// Resolve the Claude Code `projects/` directory.
    /// Precedence: `CCR_CLAUDE_DIR` > `CLAUDE_CONFIG_DIR` + `/projects` > `~/.claude/projects`.
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
        let mut files = Vec::new();
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
                files.push(p);
            }
        }
        // Parallel parse — dominates on NFS / shared filesystems where
        // per-file latency is the bottleneck (HPC home dirs, etc.).
        let out: Vec<Session> = files
            .par_iter()
            .filter_map(|p| {
                let id = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .filter(|s| !s.is_empty())?;
                let file = fs::File::open(p).ok()?;
                parse_session_from_reader(id, p.clone(), BufReader::new(file)).ok()
            })
            .collect();
        Ok(out)
    }

    fn resume(&self, s: &Session) -> Command {
        let mut cmd = Command::new("claude");
        cmd.arg("--resume").arg(&s.id).current_dir(&s.cwd);
        cmd
    }

    fn all_turns(&self, s: &Session) -> Result<Vec<Turn>> {
        let file =
            fs::File::open(&s.origin).with_context(|| format!("open {}", s.origin.display()))?;
        let reader = BufReader::new(file);
        let mut turns = Vec::new();
        for line in reader.lines() {
            let Ok(line) = line else { continue };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(role) = v.get("type").and_then(|t| t.as_str()).and_then(Role::parse) else {
                continue;
            };
            let Some(content) = v.get("message").and_then(|m| m.get("content")) else {
                continue;
            };
            let text = extract_text(content);
            if text.trim().is_empty() {
                continue;
            }
            turns.push(Turn { role, text });
        }
        Ok(turns)
    }
}

fn extract_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.to_string(),
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

pub(crate) fn parse_session_from_reader(
    id: &str,
    origin: PathBuf,
    reader: impl BufRead,
) -> Result<Session> {
    let mut cwd: Option<PathBuf> = None;
    let mut title: Option<String> = None;
    let mut last_ts: Option<DateTime<Local>> = None;
    let mut message_count = 0usize;
    let mut turns: VecDeque<Turn> = VecDeque::with_capacity(PREVIEW_TURNS);
    let mut searchable = String::new();

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

        let Some(role) = v.get("type").and_then(|t| t.as_str()).and_then(Role::parse) else {
            continue;
        };

        let Some(content) = v.get("message").and_then(|m| m.get("content")) else {
            continue;
        };
        let text = extract_text(content);
        if text.trim().is_empty() {
            continue;
        }
        message_count += 1;
        if role == Role::User {
            title = Some(truncate(&text, TITLE_MAX));
        }
        append_searchable(&mut searchable, &text);
        if turns.len() == PREVIEW_TURNS {
            turns.pop_front();
        }
        turns.push_back(Turn { role, text });
    }

    let cwd = cwd.unwrap_or_else(|| PathBuf::from("(unknown)"));
    let title = title.unwrap_or_else(|| "(no user message)".into());
    let last_activity = last_ts.unwrap_or_else(|| Local.timestamp_opt(0, 0).unwrap());

    Ok(Session {
        backend: ClaudeBackend::NAME,
        id: id.to_string(),
        cwd,
        title,
        last_activity,
        message_count: Some(message_count),
        preview: turns.into_iter().collect(),
        possibly_live: is_possibly_live(last_activity),
        origin,
        searchable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn parse(jsonl: &str) -> Session {
        parse_session_from_reader("abc-123", PathBuf::from("<test>"), Cursor::new(jsonl))
            .expect("parse ok")
    }

    #[test]
    fn last_user_message_becomes_title() {
        let jsonl = r#"{"type":"user","cwd":"/home/me/proj","timestamp":"2026-04-19T10:00:00Z","message":{"content":"hello world"}}
{"type":"assistant","timestamp":"2026-04-19T10:00:01Z","message":{"content":"hi back"}}
{"type":"user","cwd":"/home/me/proj","timestamp":"2026-04-19T10:00:02Z","message":{"content":"follow up question"}}
"#;
        let s = parse(jsonl);
        assert_eq!(s.id, "abc-123");
        assert_eq!(s.backend, "claude");
        assert_eq!(s.cwd, PathBuf::from("/home/me/proj"));
        assert_eq!(s.title, "follow up question");
        assert_eq!(s.message_count, Some(3));
        assert_eq!(s.preview.len(), 3);
        assert_eq!(s.preview[0].role, Role::User);
        assert_eq!(s.preview[1].role, Role::Assistant);
        assert_eq!(s.preview[2].role, Role::User);
    }

    #[test]
    fn empty_content_is_skipped_for_title() {
        let jsonl = r#"{"type":"user","cwd":"/x","timestamp":"2026-04-19T10:00:00Z","message":{"content":""}}
{"type":"user","cwd":"/x","timestamp":"2026-04-19T10:00:01Z","message":{"content":"real"}}
"#;
        let s = parse(jsonl);
        assert_eq!(s.title, "real");
        assert_eq!(s.message_count, Some(1));
    }

    #[test]
    fn array_content_extracts_text_parts() {
        let jsonl = r#"{"type":"user","cwd":"/x","timestamp":"2026-04-19T10:00:00Z","message":{"content":[{"type":"text","text":"hi there"},{"type":"image","source":"..."}]}}
"#;
        let s = parse(jsonl);
        assert_eq!(s.title, "hi there");
    }

    #[test]
    fn no_user_messages_gives_placeholder_title() {
        let jsonl = r#"{"type":"permission-mode","sessionId":"x"}
{"type":"file-history-snapshot"}
"#;
        let s = parse(jsonl);
        assert_eq!(s.title, "(no user message)");
        assert_eq!(s.message_count, Some(0));
    }

    #[test]
    fn malformed_lines_do_not_abort_parse() {
        let jsonl = "not json\n\n{\"type\":\"user\",\"cwd\":\"/x\",\"timestamp\":\"2026-04-19T10:00:00Z\",\"message\":{\"content\":\"ok\"}}\n";
        let s = parse(jsonl);
        assert_eq!(s.title, "ok");
        assert_eq!(s.message_count, Some(1));
    }

    #[test]
    fn long_title_is_truncated() {
        let long = "x".repeat(200);
        let jsonl = format!(
            r#"{{"type":"user","cwd":"/x","timestamp":"2026-04-19T10:00:00Z","message":{{"content":"{long}"}}}}
"#
        );
        let s = parse(&jsonl);
        assert!(s.title.chars().count() <= TITLE_MAX + 1);
        assert!(s.title.ends_with('…'));
    }

    #[test]
    fn cwd_comes_from_first_record_that_has_it() {
        let jsonl = r#"{"type":"permission-mode","sessionId":"x"}
{"type":"user","cwd":"/first","timestamp":"2026-04-19T10:00:00Z","message":{"content":"hi"}}
{"type":"user","cwd":"/second","timestamp":"2026-04-19T10:00:01Z","message":{"content":"hey"}}
"#;
        let s = parse(jsonl);
        assert_eq!(s.cwd, PathBuf::from("/first"));
    }

    #[test]
    fn preview_is_capped_at_last_preview_turns() {
        let mut jsonl = String::new();
        for i in 0..20 {
            jsonl.push_str(&format!(
                r#"{{"type":"user","cwd":"/x","timestamp":"2026-04-19T10:00:00Z","message":{{"content":"msg {i}"}}}}
"#
            ));
        }
        let s = parse(&jsonl);
        assert_eq!(s.message_count, Some(20));
        assert_eq!(s.preview.len(), PREVIEW_TURNS);
        assert_eq!(s.preview.last().unwrap().text, "msg 19");
    }
}
