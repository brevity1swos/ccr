use chrono::{Local, TimeZone};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::session::{PLACEHOLDER_TITLE, Session};

/// Initial tail window. Title (last user message) is normally within this.
pub(crate) const TAIL_WINDOW_INITIAL: u64 = 64 * 1024;
/// Hard cap on window growth when hunting for the last user message.
pub(crate) const TAIL_WINDOW_MAX: u64 = 4 * 1024 * 1024;

/// Read up to `window` bytes from the end of `path`, returning only complete
/// lines. When the window starts mid-file, the partial leading line is dropped
/// so callers always parse whole records. Returns `(text, reached_start)` where
/// `reached_start` is true when the window covers the file from byte 0.
pub fn read_tail(path: &Path, window: u64) -> std::io::Result<(String, bool)> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(window);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.read_to_end(&mut buf)?;
    let reached_start = start == 0;
    let text = if reached_start {
        String::from_utf8_lossy(&buf).into_owned()
    } else {
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => String::from_utf8_lossy(&buf[i + 1..]).into_owned(),
            None => String::new(),
        }
    };
    Ok((text, reached_start))
}

/// Scan a session file by reading a growing tail window until `build` resolves
/// the title, the whole file has been read, or the window reaches `max`.
///
/// `build` receives the window's complete-line text and whether it reached the
/// file start, and returns a candidate `Session` (whose `message_count` is a
/// window-local count). This helper owns the parts shared by every tail-read
/// backend: window growth, the termination guard, nulling `message_count`
/// (a partial window can't count the full total), and the mtime fallback when
/// the window carried no timestamp.
pub(crate) fn scan_windowed(
    path: &Path,
    initial: u64,
    max: u64,
    build: impl Fn(&str, bool) -> Option<Session>,
) -> Option<Session> {
    let mut window = initial;
    loop {
        let (text, reached_start) = read_tail(path, window).ok()?;
        let mut s = build(&text, reached_start)?;
        let found_title = s.title != PLACEHOLDER_TITLE;
        if found_title || reached_start || window >= max {
            s.message_count = None;
            if s.last_activity == Local.timestamp_opt(0, 0).unwrap() {
                s.last_activity = crate::util::file_mtime(path);
                s.possibly_live = crate::util::is_possibly_live(s.last_activity);
            }
            return Some(s);
        }
        window = window.saturating_mul(4).min(max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str, contents: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("ccr-tail-test-{name}"));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        p
    }

    fn placeholder_session(origin: &std::path::Path) -> Session {
        Session {
            backend: "test",
            id: "id".into(),
            cwd: std::path::PathBuf::from("/x"),
            title: PLACEHOLDER_TITLE.into(),
            last_activity: Local::now(),
            message_count: Some(3),
            preview: Vec::new(),
            possibly_live: false,
            origin: origin.to_path_buf(),
            searchable: String::new(),
        }
    }

    #[test]
    fn scan_windowed_terminates_at_max_when_title_never_resolves() {
        // A file larger than `max` whose `build` never resolves the title must
        // terminate via the `window >= max` guard, not loop forever re-reading
        // the capped window.
        let p = tmp("scan-windowed-noresolve", &"x".repeat(500));
        let calls = std::cell::Cell::new(0u32);
        let s = scan_windowed(&p, 16, 64, |_text, _reached| {
            calls.set(calls.get() + 1);
            Some(placeholder_session(&p))
        })
        .expect("terminates with a session");
        assert_eq!(s.title, PLACEHOLDER_TITLE);
        assert_eq!(s.message_count, None); // helper nulls the window-local count
        assert!((2..=5).contains(&calls.get())); // bounded: 16 -> 64 -> stop
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn scan_windowed_stops_when_build_resolves_title() {
        let p = tmp("scan-windowed-resolve", &"x".repeat(500));
        let calls = std::cell::Cell::new(0u32);
        let s = scan_windowed(&p, 16, 64, |_text, _reached| {
            calls.set(calls.get() + 1);
            let mut sess = placeholder_session(&p);
            sess.title = "resolved".into();
            Some(sess)
        })
        .expect("session");
        assert_eq!(s.title, "resolved");
        assert_eq!(calls.get(), 1); // resolved on first window, no growth
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn small_file_returns_whole_and_reached_start() {
        let p = tmp("small", "a\nb\nc\n");
        let (text, reached) = read_tail(&p, 1024).unwrap();
        assert_eq!(text, "a\nb\nc\n");
        assert!(reached);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn window_smaller_than_file_drops_partial_leading_line() {
        // 5 lines of "lineN\n"; a tiny window lands mid-file.
        let p = tmp("partial", "line0\nline1\nline2\nline3\nline4\n");
        // window 12 bytes ~ covers "ine4\n" plus part of "line3\n"
        let (text, reached) = read_tail(&p, 12).unwrap();
        assert!(!reached);
        // No partial line: result must start at a line boundary.
        assert!(!text.contains("ine3"));
        assert!(text.ends_with("line4\n"));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn missing_file_is_err() {
        let p = std::path::Path::new("/no/such/ccr/file.jsonl");
        assert!(read_tail(p, 1024).is_err());
    }
}
