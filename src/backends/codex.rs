use anyhow::{Context, Result};
use chrono::{DateTime, Local, TimeZone};
use serde_json::Value;
use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Cursor};
use std::path::{Path, PathBuf};
use std::process::Command;

use rayon::prelude::*;

use crate::backends::Backend;
use crate::session::{
    PLACEHOLDER_TITLE, PREVIEW_TURNS, Role, Session, TITLE_MAX, Turn, append_searchable,
};
use crate::util::{is_possibly_live, truncate};

pub struct CodexBackend;

impl CodexBackend {
    const NAME: &'static str = "codex";

    /// Resolve the Codex `sessions/` directory.
    ///
    /// Precedence:
    /// 1. `CCR_CODEX_DIR` — full path to the `sessions/` dir
    /// 2. `~/.codex/sessions` — default
    fn sessions_dir() -> Result<PathBuf> {
        if let Ok(dir) = std::env::var("CCR_CODEX_DIR") {
            return Ok(PathBuf::from(dir));
        }
        let home = dirs::home_dir().context("no home dir")?;
        Ok(home.join(".codex").join("sessions"))
    }
}

impl Backend for CodexBackend {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn scan(&self) -> Result<Vec<Session>> {
        let root = Self::sessions_dir()?;
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut files = Vec::new();
        walk_jsonl(&root, &mut files)?;
        // Parallel parse — see claude backend comment on HPC latency.
        let out: Vec<Session> = files.par_iter().filter_map(|p| scan_one(p)).collect();
        Ok(out)
    }

    fn resume(&self, s: &Session) -> Command {
        let mut cmd = Command::new("codex");
        cmd.arg("resume").arg(&s.id).current_dir(&s.cwd);
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
            if v.get("type").and_then(|t| t.as_str()) != Some("response_item") {
                continue;
            }
            let payload = v.get("payload").unwrap_or(&Value::Null);
            if payload.get("type").and_then(|t| t.as_str()) != Some("message") {
                continue;
            }
            let Some(role) = payload
                .get("role")
                .and_then(|r| r.as_str())
                .and_then(Role::parse)
            else {
                continue;
            };
            let content = payload.get("content").unwrap_or(&Value::Null);
            let text = extract_codex_text(content);
            if text.trim().is_empty() || is_system_prefix(&text) {
                continue;
            }
            turns.push(Turn { role, text });
        }
        Ok(turns)
    }
}

fn walk_jsonl(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if entry.file_type()?.is_dir() {
            walk_jsonl(&p, out)?;
        } else if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(p);
        }
    }
    Ok(())
}

/// Read the first line (Codex `session_meta` with id + cwd).
fn read_head_line(path: &Path) -> Option<String> {
    let f = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(f);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    if line.trim().is_empty() {
        None
    } else {
        Some(line)
    }
}

/// Parse one Codex session from head meta + a bounded tail window.
fn scan_one(path: &Path) -> Option<Session> {
    let head = read_head_line(path)?;
    crate::tail::scan_windowed(
        path,
        crate::tail::TAIL_WINDOW_INITIAL,
        crate::tail::TAIL_WINDOW_MAX,
        |tail, reached_start| {
            // Prepend head so `session_meta` (id+cwd) is always present. When
            // reached_start the head line is already inside the tail window, so
            // skip the prepend to avoid a duplicate session_meta and the extra
            // allocation.
            let parsed = if reached_start {
                parse_session_from_reader(Cursor::new(tail), path.to_path_buf())
            } else {
                let combined = format!("{head}\n{tail}");
                parse_session_from_reader(Cursor::new(combined.as_str()), path.to_path_buf())
            };
            parsed.ok().flatten()
        },
    )
}

pub(crate) fn parse_session_from_reader(
    reader: impl BufRead,
    origin: PathBuf,
) -> Result<Option<Session>> {
    let mut id: Option<String> = None;
    let mut cwd: Option<PathBuf> = None;
    let mut last_ts: Option<DateTime<Local>> = None;
    let mut title: Option<String> = None;
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

        if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str())
            && let Ok(parsed) = DateTime::parse_from_rfc3339(ts)
        {
            last_ts = Some(parsed.with_timezone(&Local));
        }

        let record_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let payload = v.get("payload").unwrap_or(&Value::Null);

        match record_type {
            "session_meta" => {
                if let Some(session_id) = payload.get("id").and_then(|i| i.as_str()) {
                    id = Some(session_id.to_string());
                }
                if let Some(c) = payload.get("cwd").and_then(|c| c.as_str()) {
                    cwd = Some(PathBuf::from(c));
                }
            }
            "response_item" => {
                if payload.get("type").and_then(|t| t.as_str()) != Some("message") {
                    continue;
                }
                let role_str = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
                let Some(role) = Role::parse(role_str) else {
                    continue;
                };
                let content = payload.get("content").unwrap_or(&Value::Null);
                let text = extract_codex_text(content);
                if text.trim().is_empty() || is_system_prefix(&text) {
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
            _ => {}
        }
    }

    let Some(id) = id else { return Ok(None) };
    let cwd = cwd.unwrap_or_else(|| PathBuf::from("(unknown)"));
    let title = title.unwrap_or_else(|| PLACEHOLDER_TITLE.into());
    let last_activity = last_ts.unwrap_or_else(|| Local.timestamp_opt(0, 0).unwrap());

    Ok(Some(Session {
        backend: CodexBackend::NAME,
        id,
        cwd,
        title,
        last_activity,
        message_count: Some(message_count),
        preview: turns.into_iter().collect(),
        possibly_live: is_possibly_live(last_activity),
        origin,
        searchable,
    }))
}

fn extract_codex_text(content: &Value) -> String {
    let Value::Array(arr) = content else {
        return String::new();
    };
    arr.iter()
        .filter_map(|c| match c.get("type").and_then(|t| t.as_str()) {
            Some("input_text") | Some("output_text") | Some("text") => {
                c.get("text").and_then(|t| t.as_str()).map(String::from)
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_system_prefix(text: &str) -> bool {
    text.starts_with("<environment_context>") || text.starts_with("<permissions instructions>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn parse(jsonl: &str) -> Option<Session> {
        parse_session_from_reader(Cursor::new(jsonl), PathBuf::from("<test>")).unwrap()
    }

    #[test]
    fn extracts_session_id_and_cwd_from_meta() {
        let jsonl = r#"{"type":"session_meta","timestamp":"2026-04-01T19:28:35.898Z","payload":{"id":"abc-123","cwd":"/my/proj","timestamp":"2026-04-01T19:28:35.898Z"}}
{"type":"response_item","timestamp":"2026-04-01T19:28:40Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}
"#;
        let s = parse(jsonl).expect("session");
        assert_eq!(s.id, "abc-123");
        assert_eq!(s.cwd, PathBuf::from("/my/proj"));
        assert_eq!(s.title, "hello");
        assert_eq!(s.backend, "codex");
    }

    #[test]
    fn skips_environment_and_permissions_blocks() {
        let jsonl = r#"{"type":"session_meta","timestamp":"2026-04-01T19:28:35.898Z","payload":{"id":"abc","cwd":"/x"}}
{"type":"response_item","timestamp":"2026-04-01T19:28:40Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>cwd=/x</environment_context>"}]}}
{"type":"response_item","timestamp":"2026-04-01T19:28:41Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"real question"}]}}
"#;
        let s = parse(jsonl).expect("session");
        assert_eq!(s.title, "real question");
    }

    #[test]
    fn no_meta_means_no_session() {
        let jsonl = r#"{"type":"response_item","timestamp":"2026-04-01T19:28:40Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}}
"#;
        assert!(parse(jsonl).is_none());
    }

    #[test]
    fn scan_one_uses_head_meta_and_tail_turns() {
        use std::io::Write;
        let mut path = std::env::temp_dir();
        path.push("ccr-codex-scan-one.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"session_meta","timestamp":"2026-04-01T19:28:35Z","payload":{{"id":"sid-9","cwd":"/my/proj"}}}}"#
        )
        .unwrap();
        for i in 0..40 {
            writeln!(
                f,
                r#"{{"type":"response_item","timestamp":"2026-04-01T19:29:0{}Z","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"q {}"}}]}}}}"#,
                i % 10, i
            )
            .unwrap();
        }
        let s = scan_one(&path).expect("session");
        assert_eq!(s.id, "sid-9");
        assert_eq!(s.cwd, std::path::PathBuf::from("/my/proj"));
        assert!(s.title.starts_with("q "));
        assert_eq!(s.message_count, None);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn developer_role_is_skipped() {
        let jsonl = r#"{"type":"session_meta","timestamp":"2026-04-01T19:28:35.898Z","payload":{"id":"abc","cwd":"/x"}}
{"type":"response_item","timestamp":"2026-04-01T19:28:40Z","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"system prompt"}]}}
{"type":"response_item","timestamp":"2026-04-01T19:28:41Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"actual"}]}}
"#;
        let s = parse(jsonl).expect("session");
        assert_eq!(s.title, "actual");
        assert_eq!(s.message_count, Some(1));
    }
}
