use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

pub fn bookmarks_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("CCR_BOOKMARKS_FILE") {
        return Ok(PathBuf::from(p));
    }
    let home = dirs::home_dir().context("no home dir")?;
    Ok(home.join(".ccr").join("bookmarks.json"))
}

/// Best-effort load. Returns empty set if the file is missing or malformed —
/// bookmarks are user-convenience state, not correctness-critical.
pub fn load() -> HashSet<String> {
    let Ok(path) = bookmarks_path() else {
        return HashSet::new();
    };
    load_from(&path)
}

fn save_to(bookmarks: &HashSet<String>, path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    let mut list: Vec<&str> = bookmarks.iter().map(|s| s.as_str()).collect();
    list.sort();
    crate::util::write_atomic(path, &serde_json::to_string_pretty(&list)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn load_from(path: &std::path::Path) -> HashSet<String> {
    let Ok(content) = fs::read_to_string(path) else {
        return HashSet::new();
    };
    serde_json::from_str::<Vec<String>>(&content)
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

/// Add if absent, remove if present, then persist. If the save fails the
/// in-memory set is restored, so the UI never shows a change that was lost.
pub fn toggle(bookmarks: &mut HashSet<String>, id: &str) -> Result<bool> {
    toggle_in(bookmarks, id, &bookmarks_path()?)
}

fn toggle_in(bookmarks: &mut HashSet<String>, id: &str, path: &std::path::Path) -> Result<bool> {
    let was_present = bookmarks.remove(id);
    if !was_present {
        bookmarks.insert(id.to_string());
    }
    if let Err(e) = save_to(bookmarks, path) {
        if was_present {
            bookmarks.insert(id.to_string());
        } else {
            bookmarks.remove(id);
        }
        return Err(e);
    }
    Ok(!was_present) // returns new state: true == now bookmarked
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn tmp_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "ccr-bm-test-{}-{}.json",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ))
    }

    #[test]
    fn load_missing_file_returns_empty() {
        assert!(load_from(&tmp_path()).is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let p = tmp_path();
        let mut s = HashSet::new();
        s.insert("abc".into());
        s.insert("def".into());
        save_to(&s, &p).unwrap();
        assert_eq!(load_from(&p), s);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn toggle_adds_then_removes() {
        let p = tmp_path();
        let mut s = HashSet::new();
        assert!(toggle_in(&mut s, "xyz", &p).unwrap());
        assert!(s.contains("xyz"));
        assert!(!toggle_in(&mut s, "xyz", &p).unwrap());
        assert!(!s.contains("xyz"));
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn failed_save_restores_previous_state() {
        // Parent "directory" is a regular file, so the save cannot succeed.
        let blocker = tmp_path();
        fs::write(&blocker, "").unwrap();
        let p = blocker.join("bookmarks.json");
        let mut s: HashSet<String> = ["kept".to_string()].into();
        assert!(toggle_in(&mut s, "new", &p).is_err());
        assert!(!s.contains("new"));
        assert!(toggle_in(&mut s, "kept", &p).is_err());
        assert!(s.contains("kept"));
        let _ = fs::remove_file(&blocker);
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let p = tmp_path();
        save_to(&["a".to_string()].into(), &p).unwrap();
        let tmp = format!("{}.tmp.{}", p.display(), std::process::id());
        assert!(!std::path::Path::new(&tmp).exists());
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn load_malformed_json_returns_empty() {
        let p = tmp_path();
        fs::write(&p, "not valid json").unwrap();
        assert!(load_from(&p).is_empty());
        let _ = fs::remove_file(&p);
    }
}
