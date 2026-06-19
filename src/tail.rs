use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Read up to `window` bytes from the end of `path`, returning only complete
/// lines. When the window starts mid-file, the partial leading line is dropped
/// so callers always parse whole records. Returns `(text, reached_start)` where
/// `reached_start` is true when the window covers the file from byte 0.
#[allow(dead_code)]
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
