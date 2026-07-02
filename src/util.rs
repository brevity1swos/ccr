use chrono::{DateTime, Local, TimeZone, Utc};
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

/// Resume flag / subcommand tokens that a live attach places directly before the
/// session id: `claude --resume <id>` / `claude -r <id>` / `codex resume <id>`.
const RESUME_FLAGS: [&str; 3] = ["--resume", "-r", "resume"];

/// pgrep flags that print `PID full-cmdline` lines, which differ by platform:
/// procps (Linux) uses `-a` for "list full command line", but BSD/macOS `-a`
/// means "include process ancestors" and prints bare PIDs — there, `-l`
/// combined with `-f` prints the full argument list instead. Using the wrong
/// form silently disables the live-session guard (bare-PID lines carry no
/// argv for [`line_resumes_session`] to match).
#[cfg(target_os = "linux")]
const PGREP_LIST_ARGS: [&str; 1] = ["-af"];
#[cfg(not(target_os = "linux"))]
const PGREP_LIST_ARGS: [&str; 1] = ["-fl"];

/// Processes that appear to be *resuming this exact session*.
///
/// A pgrep prefilter (platform flags: [`PGREP_LIST_ARGS`]) is refined to only
/// the lines where the id is the
/// argument of a resume flag/subcommand (`--resume <id>`, `-r <id>`,
/// `resume <id>`, or the fused `--resume=<id>` form). A bare substring match is
/// not enough — that would flag any process that merely mentions the id: an
/// editor with `<id>.jsonl` open, `tail -f …/<id>.jsonl`, another `ccr`
/// subcommand, or a shell line that names it. Returns `pid cmdline` strings.
/// Empty on non-Unix or no match.
pub fn pgrep_session(id: &str) -> Vec<String> {
    let own_pid = std::process::id().to_string();
    let mut args: Vec<&str> = PGREP_LIST_ARGS.to_vec();
    args.push(id);
    let Ok(out) = Command::new("pgrep").args(&args).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(sanitize_line)
        .filter(|l| line_resumes_session(l, id, &own_pid))
        .collect()
}

/// Replace control bytes — and Unicode bidi controls, which reorder rendered
/// text — with spaces. pgrep output embeds other processes' argv verbatim;
/// argv is attacker-controlled on shared hosts, and these lines are re-printed
/// to the user's terminal (the `ccr resume` refusal message and the TUI
/// confirm modal), so neither ESC/CSI bytes nor visual-spoofing overrides may
/// pass through.
fn sanitize_line(line: &str) -> String {
    line.chars()
        .map(|c| {
            let bidi = matches!(
                c,
                '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
            );
            if c.is_control() || bidi { ' ' } else { c }
        })
        .collect()
}

/// True when a `PID full-cmdline` pgrep line (`<pid> <arg0> <arg1> …`) is a live attach to
/// `id`: not our own PID, not a `ccr` process, and carries the id as a resume
/// argument. Split out from [`pgrep_session`] so the matching logic is testable
/// without spawning `pgrep`.
fn line_resumes_session(line: &str, id: &str, own_pid: &str) -> bool {
    let mut parts = line.split_whitespace();
    let Some(pid) = parts.next() else {
        return false;
    };
    if pid == own_pid {
        return false;
    }
    let Some(prog) = parts.next() else {
        return false;
    };
    // Never flag ccr's own processes (this picker, `ccr resume`, `ccr export`, …).
    if prog == "ccr" || prog.ends_with("/ccr") {
        return false;
    }
    let args: Vec<&str> = parts.collect();
    resume_arg_present(&args, id)
}

/// True when `id` appears in `args` as the value of a resume flag — either the
/// separate form (`--resume <id>`) or the fused form (`--resume=<id>`).
fn resume_arg_present(args: &[&str], id: &str) -> bool {
    args.iter().enumerate().any(|(i, tok)| {
        if let Some((flag, val)) = tok.split_once('=')
            && RESUME_FLAGS.contains(&flag)
            && val == id
        {
            return true;
        }
        *tok == id && i > 0 && RESUME_FLAGS.contains(&args[i - 1])
    })
}

/// File modification time as `DateTime<Local>`, or the unix epoch when the
/// file is missing or has no mtime. Used as a last-resort `last_activity`
/// when a tail window yields no message timestamp.
pub fn file_mtime(path: &Path) -> DateTime<Local> {
    match std::fs::metadata(path).and_then(|m| m.modified()) {
        Ok(t) => DateTime::<Utc>::from(t).with_timezone(&Local),
        Err(_) => Local.timestamp_opt(0, 0).unwrap(),
    }
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
    fn pgrep_session_returns_empty_for_improbable_pattern() {
        assert!(pgrep_session("!!!definitely-not-a-real-process-pattern-xyz!!!").is_empty());
    }

    const ID: &str = "abc-123";

    #[test]
    fn matches_claude_and_codex_resume_argv() {
        assert!(line_resumes_session(
            &format!("42318 claude --resume {ID}"),
            ID,
            "1"
        ));
        assert!(line_resumes_session(
            &format!("42318 claude -r {ID}"),
            ID,
            "1"
        ));
        assert!(line_resumes_session(
            &format!("42318 /usr/bin/codex resume {ID}"),
            ID,
            "1"
        ));
        assert!(line_resumes_session(
            &format!("42318 claude --resume={ID}"),
            ID,
            "1"
        ));
    }

    #[test]
    fn ignores_bare_mentions_of_the_id() {
        // editor / tail with the session file open — id is a substring of the
        // filename token, not a resume argument
        assert!(!line_resumes_session(
            &format!("42318 nvim /home/me/.claude/projects/x/{ID}.jsonl"),
            ID,
            "1"
        ));
        assert!(!line_resumes_session(
            &format!("42318 tail -f /x/{ID}.jsonl"),
            ID,
            "1"
        ));
        // a shell line that names the id but is not resuming it
        assert!(!line_resumes_session(
            &format!("42318 grep {ID} log.txt"),
            ID,
            "1"
        ));
    }

    #[test]
    fn ignores_own_pid_and_ccr_processes() {
        assert!(!line_resumes_session(
            &format!("77 claude --resume {ID}"),
            ID,
            "77"
        ));
        assert!(!line_resumes_session(
            &format!("42318 ccr resume {ID}"),
            ID,
            "1"
        ));
        assert!(!line_resumes_session(
            &format!("42318 /opt/bin/ccr resume {ID}"),
            ID,
            "1"
        ));
    }

    #[test]
    fn resume_arg_present_requires_flag_before_id() {
        assert!(resume_arg_present(&["--resume", ID], ID));
        assert!(resume_arg_present(&["resume", ID], ID));
        assert!(resume_arg_present(&["--resume=abc-123"], ID));
        assert!(!resume_arg_present(&[ID], ID)); // id with no preceding flag
        assert!(!resume_arg_present(&["--other", ID], ID));
        assert!(!resume_arg_present(&["--resume", "other-id"], ID));
    }

    #[test]
    fn fused_short_and_bare_forms_match() {
        assert!(resume_arg_present(&["-r=abc-123"], ID));
        assert!(resume_arg_present(&["resume=abc-123"], ID));
        assert!(!resume_arg_present(&["-r=other-id"], ID));
    }

    // These false positives are ACCEPTED by design: the matcher deliberately
    // ignores the program name (node-shim installs run as `node …/cli.js
    // --resume <id>`), so any process with a resume-shaped token pair is
    // flagged. The failure direction is safe — a spurious refusal that
    // `--force` (CLI) or the confirm modal (TUI) overrides — whereas requiring
    // known program names would silently miss real attaches.
    #[test]
    fn accepted_false_positives_are_documented() {
        // `grep -r <id> <dir>`: "-r" happens to be a resume flag token.
        assert!(line_resumes_session(
            &format!("42318 grep -r {ID} /var/log"),
            ID,
            "1"
        ));
        // pgrep space-joins argv without quoting, so a single text argument
        // *containing* "--resume <id>" tokenizes into a phantom flag+id pair.
        assert!(line_resumes_session(
            &format!("42318 claude -p why does --resume {ID} hang"),
            ID,
            "1"
        ));
    }

    #[test]
    fn sanitize_line_replaces_control_bytes() {
        assert_eq!(
            sanitize_line("42 claude --resume \x1b[2Jabc\x07"),
            "42 claude --resume  [2Jabc "
        );
        assert_eq!(
            sanitize_line("42 claude --resume abc"),
            "42 claude --resume abc"
        );
    }

    /// Platform-shape guard: pgrep must emit `PID cmdline` lines on THIS
    /// platform or the live-session guard is silently dead (the macOS
    /// `pgrep -af` bare-PID regression). Spawns a real decoy process carrying
    /// `--resume <unique-id>` in its argv and asserts pgrep_session sees it.
    /// Requires a working pgrep + readable process table (GH-hosted runners
    /// have both; slim container images without procps would fail here — by
    /// design, since the guard is equally dead there).
    #[test]
    #[cfg(unix)]
    fn live_check_detects_synthetic_resume_process() {
        let id = format!("ccr-live-check-test-{}", std::process::id());
        // "; :" keeps this a compound command — a single command would be
        // exec'd directly by sh, replacing the argv that carries `--resume`.
        let mut decoy = Command::new("sh")
            .args(["-c", "sleep 30; :", "decoy-argv0", "--resume", &id])
            .spawn()
            .expect("spawn decoy");
        // pgrep needs the process visible; poll briefly instead of one sleep.
        let mut found = Vec::new();
        for _ in 0..25 {
            found = pgrep_session(&id);
            if !found.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        decoy.kill().ok();
        decoy.wait().ok();
        assert!(
            !found.is_empty(),
            "pgrep_session must detect a live `--resume {id}` process on this platform \
             (wrong pgrep output shape? see PGREP_LIST_ARGS)"
        );
    }

    #[test]
    fn file_mtime_of_existing_file_is_recent() {
        use std::io::Write;
        let mut p = std::env::temp_dir();
        p.push("ccr-mtime-test");
        std::fs::File::create(&p).unwrap().write_all(b"x").unwrap();
        let mt = file_mtime(&p);
        // within a day of now (loose; just proves it read a real mtime)
        assert!((Local::now() - mt).num_seconds().abs() < 86_400);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn file_mtime_of_missing_file_is_epoch() {
        let mt = file_mtime(std::path::Path::new("/no/such/ccr/path"));
        assert_eq!(mt.timestamp(), 0);
    }
}
