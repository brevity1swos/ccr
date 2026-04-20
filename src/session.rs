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
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
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
}
