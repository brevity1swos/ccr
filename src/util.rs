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
            let mut parts = l.split_whitespace();
            let pid = parts.next().unwrap_or("");
            let cmd = parts.next().unwrap_or("");
            pid != own_pid && cmd != "ccr" && !cmd.ends_with("/ccr")
        })
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use std::path::PathBuf;

    #[test]
    fn truncate_under_max_is_passthrough() {
        assert_eq!(truncate("hi", 10), "hi");
    }

    #[test]
    fn truncate_exact_max_is_passthrough() {
        assert_eq!(truncate("abcde", 5), "abcde");
    }

    #[test]
    fn truncate_over_max_adds_ellipsis() {
        assert_eq!(truncate("abcdef", 3), "abc…");
    }

    #[test]
    fn truncate_normalizes_newlines_and_carriage_returns() {
        assert_eq!(truncate("a\nb\rc", 10), "a b c");
    }

    #[test]
    fn truncate_counts_unicode_by_char_not_byte() {
        assert_eq!(truncate("한국어입니다", 3), "한국어…");
    }

    #[test]
    fn relative_time_seconds() {
        let dt = Local::now() - Duration::seconds(10);
        assert!(relative_time(dt).ends_with("s ago"));
    }

    #[test]
    fn relative_time_minutes() {
        let dt = Local::now() - Duration::minutes(30);
        assert_eq!(relative_time(dt), "30m ago");
    }

    #[test]
    fn relative_time_hours() {
        let dt = Local::now() - Duration::hours(5);
        assert_eq!(relative_time(dt), "5h ago");
    }

    #[test]
    fn relative_time_days() {
        let dt = Local::now() - Duration::days(3);
        assert_eq!(relative_time(dt), "3d ago");
    }

    #[test]
    fn relative_time_beyond_a_month_renders_date() {
        let dt = Local::now() - Duration::days(60);
        let s = relative_time(dt);
        assert!(s.len() == 10 && s.chars().nth(4) == Some('-'));
    }

    #[test]
    fn project_basename_returns_last_segment() {
        assert_eq!(project_basename(&PathBuf::from("/a/b/proj")), "proj");
    }

    #[test]
    fn is_possibly_live_true_for_recent() {
        assert!(is_possibly_live(Local::now() - Duration::seconds(60)));
    }

    #[test]
    fn is_possibly_live_false_for_old() {
        assert!(!is_possibly_live(Local::now() - Duration::hours(1)));
    }

    #[test]
    fn pgrep_f_returns_empty_for_improbable_pattern() {
        assert!(pgrep_f("!!!definitely-not-a-real-process-pattern-xyz!!!").is_empty());
    }
}
