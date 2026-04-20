use anyhow::{Context, Result};
use chrono::{DateTime, Local, TimeZone};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::backends::Backend;
use crate::session::{PREVIEW_TURNS, Role, Session, TITLE_MAX, Turn};
use crate::util::{is_possibly_live, truncate};

pub struct GeminiBackend;

impl GeminiBackend {
    const NAME: &'static str = "gemini";

    /// Resolve Gemini's state root directory.
    ///
    /// Precedence:
    /// 1. `CCR_GEMINI_DIR` — full path to the state dir
    /// 2. `~/.gemini` — default
    fn root() -> Result<PathBuf> {
        if let Ok(dir) = std::env::var("CCR_GEMINI_DIR") {
            return Ok(PathBuf::from(dir));
        }
        let home = dirs::home_dir().context("no home dir")?;
        Ok(home.join(".gemini"))
    }

    fn load_project_map(root: &Path) -> HashMap<String, PathBuf> {
        let path = root.join("projects.json");
        let Ok(content) = fs::read_to_string(&path) else {
            return HashMap::new();
        };
        let Ok(v) = serde_json::from_str::<Value>(&content) else {
            return HashMap::new();
        };
        let Some(projects) = v.get("projects").and_then(|p| p.as_object()) else {
            return HashMap::new();
        };
        projects
            .iter()
            .filter_map(|(cwd, short)| short.as_str().map(|n| (n.to_string(), PathBuf::from(cwd))))
            .collect()
    }
}

impl Backend for GeminiBackend {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn scan(&self) -> Result<Vec<Session>> {
        let root = Self::root()?;
        let tmp = root.join("tmp");
        if !tmp.exists() {
            return Ok(Vec::new());
        }
        let projects = Self::load_project_map(&root);

        let mut out = Vec::new();
        for entry in fs::read_dir(&tmp).with_context(|| format!("read_dir {}", tmp.display()))? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let short_name = entry.file_name().to_string_lossy().into_owned();
            let chats = entry.path().join("chats");
            if !chats.exists() {
                continue;
            }
            let cwd = projects
                .get(&short_name)
                .cloned()
                .unwrap_or_else(|| PathBuf::from(format!("(unknown: {short_name})")));
            for f in fs::read_dir(&chats)? {
                let f = f?;
                let p = f.path();
                if p.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(Some(s)) = parse_session(&p, cwd.clone()) {
                    out.push(s);
                }
            }
        }
        Ok(out)
    }

    fn resume(&self, s: &Session) -> Command {
        // Gemini's `--resume <N>` takes a 1-based project-scoped index, not a
        // session UUID. We look up the index at resume time by grepping for
        // `[<uuid>]` in `gemini --list-sessions` output.
        let mut cmd = Command::new("sh");
        let script = format!(
            "N=$(gemini --list-sessions 2>/dev/null | grep -F '[{id}]' | sed -E 's/^ *([0-9]+)\\..*/\\1/' | head -1); \
             if [ -n \"$N\" ]; then exec gemini --resume \"$N\"; else echo 'ccr: gemini session not found for this project' >&2; exit 1; fi",
            id = s.id
        );
        cmd.arg("-c").arg(script).current_dir(&s.cwd);
        cmd
    }

    fn running(&self, _s: &Session) -> Vec<String> {
        // `gemini --resume N` doesn't embed the session UUID in argv,
        // so pgrep-by-id gives no signal — skip live-detection.
        Vec::new()
    }
}

fn parse_session(path: &Path, cwd: PathBuf) -> Result<Option<Session>> {
    let content = fs::read_to_string(path)?;
    parse_session_from_json(&content, cwd, path.to_path_buf())
}

pub(crate) fn parse_session_from_json(
    content: &str,
    cwd: PathBuf,
    origin: PathBuf,
) -> Result<Option<Session>> {
    let Ok(v) = serde_json::from_str::<Value>(content) else {
        return Ok(None);
    };
    let Some(id) = v
        .get("sessionId")
        .and_then(|i| i.as_str())
        .map(String::from)
    else {
        return Ok(None);
    };

    let ts_str = v
        .get("lastUpdated")
        .or_else(|| v.get("startTime"))
        .and_then(|t| t.as_str());
    let last_activity = ts_str
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.with_timezone(&Local))
        .unwrap_or_else(|| Local.timestamp_opt(0, 0).unwrap());

    let messages = v
        .get("messages")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();
    let mut title: Option<String> = None;
    let mut message_count = 0usize;
    let mut turns: VecDeque<Turn> = VecDeque::with_capacity(PREVIEW_TURNS);

    for msg in &messages {
        let role_str = msg.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let Some(role) = Role::parse(role_str) else {
            continue;
        };
        let content = msg.get("content").unwrap_or(&Value::Null);
        let text = extract_gemini_text(content);
        if text.trim().is_empty() {
            continue;
        }
        message_count += 1;
        if role == Role::User && title.is_none() {
            title = Some(truncate(&text, TITLE_MAX));
        }
        if turns.len() == PREVIEW_TURNS {
            turns.pop_front();
        }
        turns.push_back(Turn { role, text });
    }

    let title = title.unwrap_or_else(|| "(no user message)".into());

    Ok(Some(Session {
        backend: GeminiBackend::NAME,
        id,
        cwd,
        title,
        last_activity,
        message_count,
        preview: turns.into_iter().collect(),
        possibly_live: is_possibly_live(last_activity),
        origin,
    }))
}

fn extract_gemini_text(content: &Value) -> String {
    match content {
        Value::Array(arr) => arr
            .iter()
            .filter_map(|c| c.get("text").and_then(|t| t.as_str()).map(String::from))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::String(s) => s.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Option<Session> {
        parse_session_from_json(json, PathBuf::from("/proj"), PathBuf::from("<test>")).unwrap()
    }

    #[test]
    fn extracts_fields_from_session_json() {
        let json = r#"{
            "sessionId": "abc-123",
            "startTime": "2026-04-01T19:14:18.634Z",
            "lastUpdated": "2026-04-01T19:41:53.334Z",
            "messages": [
                {"id":"m1","timestamp":"2026-04-01T19:14:18.634Z","type":"user","content":[{"text":"hello"}]},
                {"id":"m2","timestamp":"2026-04-01T19:14:19.000Z","type":"gemini","content":[{"text":"hi back"}]}
            ]
        }"#;
        let s = parse(json).expect("session");
        assert_eq!(s.id, "abc-123");
        assert_eq!(s.title, "hello");
        assert_eq!(s.message_count, 2);
        assert_eq!(s.preview[0].role, Role::User);
        assert_eq!(s.preview[1].role, Role::Assistant);
        assert_eq!(s.backend, "gemini");
    }

    #[test]
    fn missing_session_id_returns_none() {
        let json = r#"{"startTime":"2026-04-01T19:14:18.634Z","messages":[]}"#;
        assert!(parse(json).is_none());
    }

    #[test]
    fn info_role_messages_are_skipped() {
        let json = r#"{
            "sessionId": "x",
            "lastUpdated": "2026-04-01T19:14:18.634Z",
            "messages": [
                {"type":"info","content":[{"text":"meta"}]},
                {"type":"user","content":[{"text":"real"}]}
            ]
        }"#;
        let s = parse(json).expect("session");
        assert_eq!(s.title, "real");
        assert_eq!(s.message_count, 1);
    }
}
