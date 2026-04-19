use chrono::{DateTime, Local};
use std::path::Path;
use std::process::Command;

use crate::session::LIVE_WINDOW_SECS;

pub fn truncate(s: &str, max: usize) -> String {
    let cleaned = s.replace(['\n', '\r'], " ");
    if cleaned.chars().count() <= max {
        cleaned
    } else {
        let mut out: String = cleaned.chars().take(max).collect();
        out.push('…');
        out
    }
}

pub fn relative_time(dt: DateTime<Local>) -> String {
    let delta = Local::now().signed_duration_since(dt);
    let s = delta.num_seconds();
    if s < 60 {
        format!("{s}s ago")
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else if s < 86400 {
        format!("{}h ago", s / 3600)
    } else if s < 86400 * 30 {
        format!("{}d ago", s / 86400)
    } else {
        dt.format("%Y-%m-%d").to_string()
    }
}

pub fn project_basename(p: &Path) -> String {
    p.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_string()
}

pub fn is_possibly_live(last_activity: DateTime<Local>) -> bool {
    Local::now()
        .signed_duration_since(last_activity)
        .num_seconds()
        < LIVE_WINDOW_SECS
}

/// `pgrep -af <pattern>` filtered to exclude our own PID and any `ccr` processes.
/// Returns `pid cmdline` strings. Empty on non-Unix or no match.
pub fn pgrep_f(pattern: &str) -> Vec<String> {
    let own_pid = std::process::id().to_string();
    let Ok(out) = Command::new("pgrep").args(["-af", pattern]).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| {
            let pid = l.split_whitespace().next().unwrap_or("");
            pid != own_pid && !l.contains(" ccr") && !l.ends_with("/ccr")
        })
        .map(String::from)
        .collect()
}
