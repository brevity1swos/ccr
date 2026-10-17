use anyhow::{Context, Result};
use chrono::{DateTime, Local, TimeZone};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use rayon::prelude::*;

use crate::backends::Backend;
use crate::session::{
    PLACEHOLDER_TITLE, PREVIEW_TURNS, Role, Session, TITLE_MAX, Turn, append_searchable,
};
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

        // Collect (path, cwd) pairs sequentially (directory walk), then
        // parse files in parallel — see claude backend comment.
        let mut work: Vec<(PathBuf, PathBuf)> = Vec::new();
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
                if !matches!(
                    p.extension().and_then(|e| e.to_str()),
                    Some("json" | "jsonl")
                ) {
                    continue;
                }
                work.push((p, cwd.clone()));
            }
        }
        let out: Vec<Session> = work
            .par_iter()
            .filter_map(|(p, cwd)| parse_session(p, cwd.clone()).ok().flatten())
            .collect();
        Ok(out)
    }

    fn resume(&self, s: &Session) -> Command {
        // Gemini's `--resume <N>` takes a 1-based project-scoped index, not a
        // session UUID. We look up the index at resume time by grepping for
        // `[<uuid>]` in `gemini --list-sessions` output.
        //
        // The session ID is passed as $1 (positional arg) rather than interpolated
        // into the script string, so shell metacharacters in the ID are inert.
        let mut cmd = Command::new("sh");
        let script = "N=$(gemini --list-sessions 2>/dev/null | grep -F \"[$1]\" | \
                      sed -E 's/^ *([0-9]+)\\..*/\\1/' | head -1); \
                      if [ -n \"$N\" ]; then exec gemini --resume \"$N\"; \
                      else echo 'ccr: gemini session not found for this project' >&2; exit 1; fi";
        cmd.args(["-c", script, "--", &s.id]).current_dir(&s.cwd);
        cmd
    }

    fn running(&self, _s: &Session) -> Vec<String> {
        // `gemini --resume N` doesn't embed the session UUID in argv,
        // so pgrep-by-id gives no signal — skip live-detection.
        Vec::new()
    }

    fn all_turns_at(&self, origin: &Path) -> Result<Vec<Turn>> {
        let content =
            fs::read_to_string(origin).with_context(|| format!("read {}", origin.display()))?;
        let v = parse_conversation(&content);
        let messages = v
            .get("messages")
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_default();
        let mut turns = Vec::new();
        for msg in &messages {
            let Some(role) = msg
                .get("type")
                .and_then(|t| t.as_str())
                .and_then(Role::parse)
            else {
                continue;
            };
            let content = msg.get("content").unwrap_or(&Value::Null);
            let text = extract_gemini_text(content);
            if text.trim().is_empty() {
                continue;
            }
            turns.push(Turn { role, text });
        }
        Ok(turns)
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
    let v = parse_conversation(content);
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
    let mut searchable = String::new();

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
        if role == Role::User {
            title = Some(truncate(&text, TITLE_MAX));
        }
        append_searchable(&mut searchable, &text);
        if turns.len() == PREVIEW_TURNS {
            turns.pop_front();
        }
        turns.push_back(Turn { role, text });
    }

    let title = title.unwrap_or_else(|| PLACEHOLDER_TITLE.into());

    Ok(Some(Session {
        backend: GeminiBackend::NAME,
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

/// Read legacy JSON snapshots or replay Gemini's append-only JSONL records.
/// Replacements keep their position; rewinds discard the target and later turns.
/// Ignore malformed records, including a partially written final line.
fn parse_conversation(content: &str) -> Value {
    if let Ok(snapshot) = serde_json::from_str::<Value>(content) {
        return snapshot;
    }

    let mut metadata = serde_json::Map::new();
    let mut messages: Vec<Value> = Vec::new();
    for line in content.lines() {
        let Ok(Value::Object(record)) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(id) = record.get("$rewindTo").and_then(Value::as_str) {
            let index = messages.iter().position(|m| m["id"] == id).unwrap_or(0);
            messages.truncate(index);
        } else if let Some(patch) = record.get("$patch").and_then(Value::as_object) {
            patch_message(&mut messages, patch);
            if let Some(updates) = patch.get("updates").and_then(Value::as_array) {
                for update in updates.iter().filter_map(Value::as_object) {
                    patch_message(&mut messages, update);
                }
            }
            if let Some(ids) = patch.get("removeIds").and_then(Value::as_array) {
                messages.retain(|m| !ids.contains(&m["id"]));
            }
            if let Some(ids) = patch.get("orderIds").and_then(Value::as_array) {
                // Unmentioned messages stay at the front, followed by the listed IDs.
                let mut ordered = Vec::new();
                for id in ids.iter().filter(|id| id.is_string()) {
                    if let Some(index) = messages.iter().position(|m| &m["id"] == id) {
                        ordered.push(messages.remove(index));
                    }
                }
                messages.extend(ordered);
            }
        } else if record.get("id").is_some_and(Value::is_string)
            && record.get("type").is_some_and(Value::is_string)
        {
            upsert_message(&mut messages, Value::Object(record));
        } else {
            let update = record.get("$set").and_then(Value::as_object);
            let fields = update.unwrap_or(&record);
            if let Some(snapshot) = fields.get("messages").and_then(Value::as_array) {
                if update.is_some() {
                    messages.clear();
                }
                for message in snapshot {
                    upsert_message(&mut messages, message.clone());
                }
            }
            metadata.extend(fields.clone());
        }
    }
    metadata.insert("messages".into(), Value::Array(messages));
    Value::Object(metadata)
}

fn upsert_message(messages: &mut Vec<Value>, message: Value) {
    let Some(id) = message.get("id").and_then(Value::as_str) else {
        return;
    };
    if !message.get("type").is_some_and(Value::is_string) {
        return;
    }
    if let Some(existing) = messages.iter_mut().find(|m| m["id"] == id) {
        *existing = message;
    } else {
        messages.push(message);
    }
}

fn patch_message(messages: &mut [Value], patch: &serde_json::Map<String, Value>) {
    let Some(id) = patch.get("id").and_then(Value::as_str) else {
        return;
    };
    if let Some(content) = patch.get("content")
        && let Some(message) = messages.iter_mut().find(|m| m["id"] == id)
    {
        message["content"] = content.clone();
    }
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
        assert_eq!(s.message_count, Some(2));
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
        assert_eq!(s.message_count, Some(1));
    }

    #[test]
    fn jsonl_replays_replacements_patches_and_metadata() {
        let jsonl = r#"{"sessionId":"jsonl-id","startTime":"2026-10-17T10:00:00Z"}
{"id":"u1","type":"user","content":[{"text":"first prompt"}]}
{"id":"a1","type":"gemini","content":"partial"}
{"id":"a1","type":"gemini","content":"complete"}
{"$patch":{"id":"u1","content":[{"text":"edited prompt"}]}}
{"$set":{"lastUpdated":"2026-10-17T11:00:00Z"}}
{"id":"incomplete"
"#;
        let s = parse(jsonl).unwrap();
        assert_eq!(s.id, "jsonl-id");
        assert_eq!(s.title, "edited prompt");
        assert_eq!(s.message_count, Some(2));
        assert_eq!(s.preview[1].text, "complete");
        assert!(!s.searchable.contains("partial"));
        assert_eq!(s.last_activity.timestamp(), 1792234800);
    }

    #[test]
    fn jsonl_replays_batch_edits_removals_order_and_rewinds() {
        let jsonl = r#"{"sessionId":"jsonl-id"}
{"id":"u1","type":"user","content":"one"}
{"id":"u2","type":"user","content":"two"}
{"id":"u3","type":"user","content":"three"}
{"id":"a1","type":"gemini","content":"answer"}
{"$patch":{"updates":[{"id":"u1","content":"edited"},{"id":"missing","content":"ignored"}],"removeIds":["u2"],"orderIds":["u3","a1","u1"]}}
"#;
        let s = parse(jsonl).unwrap();
        assert_eq!(s.message_count, Some(3));
        assert_eq!(s.preview[0].text, "three");
        assert_eq!(s.preview[2].text, "edited");

        let rewound = format!("{jsonl}{{\"$rewindTo\":\"a1\"}}\n");
        let s = parse(&rewound).unwrap();
        assert_eq!(s.message_count, Some(1));
        assert_eq!(s.title, "three");
        assert!(!s.searchable.contains("edited"));

        let cleared = format!("{rewound}{{\"$rewindTo\":\"missing\"}}\n");
        assert_eq!(parse(&cleared).unwrap().message_count, Some(0));
    }

    #[test]
    fn jsonl_set_messages_replaces_history() {
        let jsonl = r#"{"sessionId":"jsonl-id","messages":[{"id":"old","type":"user","content":"old history"}]}
{"$set":{"messages":[{"id":"new","type":"user","content":"replacement history"}]}}
{"id":"reply","type":"gemini","content":"new reply"}
"#;
        let s = parse(jsonl).unwrap();
        assert_eq!(s.title, "replacement history");
        assert_eq!(s.message_count, Some(2));
        assert!(!s.searchable.contains("old history"));
    }

    #[test]
    fn jsonl_without_session_metadata_is_ignored() {
        assert!(
            parse("{\"id\":\"u1\",\"type\":\"user\",\"content\":\"hello\"}\ninvalid\n").is_none()
        );
    }
}
