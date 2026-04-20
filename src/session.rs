use chrono::{DateTime, Local};
use std::path::PathBuf;

pub const TITLE_MAX: usize = 80;
pub const PREVIEW_TURNS: usize = 6;
pub const LIVE_WINDOW_SECS: i64 = 300;

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
    pub message_count: usize,
    pub preview: Vec<Turn>,
    pub possibly_live: bool,
    /// Absolute path to the on-disk file this session was parsed from.
    /// Used by `Backend::trash` to move the file without rescanning.
    pub origin: PathBuf,
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
    fn role_parse_returns_none_for_unknown() {
        assert_eq!(Role::parse("system"), None);
        assert_eq!(Role::parse(""), None);
        assert_eq!(Role::parse("User"), None);
    }
}
