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

pub fn save(bookmarks: &HashSet<String>) -> Result<()> {
    save_to(bookmarks, &bookmarks_path()?)
}

fn save_to(bookmarks: &HashSet<String>, path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    let mut list: Vec<&str> = bookmarks.iter().map(|s| s.as_str()).collect();
    list.sort();
    fs::write(path, serde_json::to_string_pretty(&list)?)
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

/// Add if absent, remove if present. Persists after mutation.
pub fn toggle(bookmarks: &mut HashSet<String>, id: &str) -> Result<bool> {
    let was_present = bookmarks.remove(id);
    if !was_present {
        bookmarks.insert(id.to_string());
    }
    save(bookmarks)?;
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
        unsafe { std::env::set_var("CCR_BOOKMARKS_FILE", &p) };
        let mut s = HashSet::new();
        assert!(toggle(&mut s, "xyz").unwrap());
        assert!(s.contains("xyz"));
        assert!(!toggle(&mut s, "xyz").unwrap());
        assert!(!s.contains("xyz"));
        unsafe { std::env::remove_var("CCR_BOOKMARKS_FILE") };
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
