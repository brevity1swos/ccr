use chrono::{DateTime, Local};
use std::path::PathBuf;

pub const TITLE_MAX: usize = 80;
pub const PREVIEW_TURNS: usize = 6;
pub const LIVE_WINDOW_SECS: i64 = 300;
/// Title used when a session has no user message yet. The tail-window scan
/// treats a title still equal to this as "not yet resolved" and keeps growing
/// the window.
pub const PLACEHOLDER_TITLE: &str = "(no user message)";
/// Max bytes of lowercase turn text retained per session for `/` content
/// filtering. Applies to both tiers of searchable text: the tail-window
/// `Session.searchable` built during scan (recent turns, available instantly)
/// and the full-history text the TUI's background warm-up adds alongside it
/// (`tui::warm_search_cache`); the filter checks both. The cap keeps the
/// oldest turns of the full text and the newest turns of the tail text, so a
/// conversation longer than the cap normally stays searchable at both ends
/// with only its middle falling out (in the extreme case of more than the cap
/// inside the tail window alone, the newest text can fall out too).
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
    /// Used to re-read the session (`all_turns_at`, export, search warm-up)
    /// without rescanning.
    pub origin: PathBuf,
    /// Turn text from the scan's tail window, lowercased, newline-joined,
    /// capped at `SEARCHABLE_CAP` bytes. The TUI's `/` filter always checks
    /// this alongside the warmed full-history text (which is capped from the
    /// oldest turn, so this covers the newest).
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
