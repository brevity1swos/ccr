use chrono::{DateTime, Local};
use std::path::PathBuf;

pub const TITLE_MAX: usize = 80;
pub const PREVIEW_TURNS: usize = 6;
pub const LIVE_WINDOW_SECS: i64 = 300;
/// Max bytes of lowercase turn text retained per session for `/` content
/// filtering. After the tail-read scan this covers the recent (tail-window)
/// turns, not the full history — full-content search is a deferred follow-up.
pub const SEARCHABLE_CAP: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    /// Recognizes role strings across supported tools:
    /// Claude = `user|assistant`, Codex = `user|assistant`,
    /// Gemini = `user|gemini`. Unknown roles (e.g. `developer`,
    /// `info`, `tool`) are filtered out.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "assistant" | "gemini" | "model" => Some(Self::Assistant),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Turn {
    pub role: Role,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub backend: &'static str,
    pub id: String,
    pub cwd: PathBuf,
    pub title: String,
    pub last_activity: DateTime<Local>,
    /// Exact user+assistant turn count, or `None` when scan used a partial
    /// tail window and the true total was not computed. Filled lazily by the
    /// TUI detail pane and by `ccr stats` via `Backend::all_turns`.
    pub message_count: Option<usize>,
    pub preview: Vec<Turn>,
    pub possibly_live: bool,
    /// Absolute path to the on-disk file this session was parsed from.
    /// Used by `Backend::trash` to move the file without rescanning.
    pub origin: PathBuf,
    /// All turn text across the session, lowercased, newline-joined, capped
    /// at `SEARCHABLE_CAP` bytes. Populated by backends during scan; used
    /// by the TUI's `/` filter for full-content search.
    pub searchable: String,
}

/// Append `text` (lowercased) to `buf`, preserving the cap. Emits a newline
/// separator before each non-empty append. Silently truncates mid-char-group
/// once the cap is reached.
pub fn append_searchable(buf: &mut String, text: &str) {
    if buf.len() >= SEARCHABLE_CAP {
        return;
    }
    if !buf.is_empty() {
        buf.push('\n');
    }
    for c in text.chars().flat_map(|c| c.to_lowercase()) {
        if buf.len() + c.len_utf8() > SEARCHABLE_CAP {
            return;
        }
        buf.push(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_parse_recognizes_user_and_assistant() {
        assert_eq!(Role::parse("user"), Some(Role::User));
        assert_eq!(Role::parse("assistant"), Some(Role::Assistant));
    }

    #[test]
    fn role_parse_accepts_gemini_variants() {
        assert_eq!(Role::parse("gemini"), Some(Role::Assistant));
        assert_eq!(Role::parse("model"), Some(Role::Assistant));
    }

    #[test]
    fn role_parse_returns_none_for_unknown() {
        assert_eq!(Role::parse("system"), None);
        assert_eq!(Role::parse(""), None);
        assert_eq!(Role::parse("User"), None);
    }

    #[test]
    fn append_searchable_lowercases() {
        let mut buf = String::new();
        append_searchable(&mut buf, "Hello WORLD");
        assert_eq!(buf, "hello world");
    }

    #[test]
    fn append_searchable_joins_with_newline() {
        let mut buf = String::new();
        append_searchable(&mut buf, "first");
        append_searchable(&mut buf, "Second");
        assert_eq!(buf, "first\nsecond");
    }

    #[test]
    fn append_searchable_respects_cap() {
        let mut buf = String::new();
        let long = "x".repeat(SEARCHABLE_CAP + 10);
        append_searchable(&mut buf, &long);
        assert_eq!(buf.len(), SEARCHABLE_CAP);
        // Further appends are no-ops.
        append_searchable(&mut buf, "yyy");
        assert_eq!(buf.len(), SEARCHABLE_CAP);
    }

    #[test]
    fn append_searchable_handles_unicode() {
        let mut buf = String::new();
        append_searchable(&mut buf, "파이썬");
        assert_eq!(buf, "파이썬");
    }
}
