use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub fn nicknames_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("CCR_NICKNAMES_FILE") {
        return Ok(PathBuf::from(p));
    }
    let home = dirs::home_dir().context("no home dir")?;
    Ok(home.join(".ccr").join("nicknames.json"))
}

pub fn load() -> HashMap<String, String> {
    let Ok(path) = nicknames_path() else {
        return HashMap::new();
    };
    load_from(&path)
}

fn save_to(nicknames: &HashMap<String, String>, path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    crate::util::write_atomic(path, &serde_json::to_string_pretty(nicknames)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn load_from(path: &std::path::Path) -> HashMap<String, String> {
    let Ok(content) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str::<HashMap<String, String>>(&content).unwrap_or_default()
}

/// Set a nickname. Empty or whitespace-only name removes the entry.
pub fn set(nicknames: &mut HashMap<String, String>, id: &str, name: &str) -> Result<()> {
    set_in(nicknames, id, name, &nicknames_path()?)
}

/// [`set`] against an explicit file. If the save fails the in-memory map is
/// restored, so the UI never shows a nickname that was not persisted.
fn set_in(
    nicknames: &mut HashMap<String, String>,
    id: &str,
    name: &str,
    path: &std::path::Path,
) -> Result<()> {
    let previous = if name.trim().is_empty() {
        nicknames.remove(id)
    } else {
        nicknames.insert(id.to_string(), name.trim().to_string())
    };
    save_to(nicknames, path).inspect_err(|_| match previous {
        Some(old) => {
            nicknames.insert(id.to_string(), old);
        }
        None => {
            nicknames.remove(id);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn tmp_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "ccr-nick-{}-{}.json",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ))
    }

    #[test]
    fn load_missing_returns_empty() {
        assert!(load_from(&tmp_path()).is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let p = tmp_path();
        let mut m = HashMap::new();
        m.insert("abc".into(), "my work".into());
        save_to(&m, &p).unwrap();
        assert_eq!(load_from(&p), m);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn set_adds_and_removes() {
        let p = tmp_path();
        let mut m = HashMap::new();
        set_in(&mut m, "abc", "hello", &p).unwrap();
        assert_eq!(m.get("abc").map(String::as_str), Some("hello"));
        set_in(&mut m, "abc", "", &p).unwrap();
        assert!(!m.contains_key("abc"));
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn set_trims_whitespace() {
        let p = tmp_path();
        let mut m = HashMap::new();
        set_in(&mut m, "abc", "  hello  ", &p).unwrap();
        assert_eq!(m.get("abc").map(String::as_str), Some("hello"));
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn failed_save_restores_previous_nickname() {
        // Parent "directory" is a regular file, so the save cannot succeed.
        let blocker = tmp_path();
        fs::write(&blocker, "").unwrap();
        let p = blocker.join("nicknames.json");
        let mut m = HashMap::new();
        m.insert("abc".to_string(), "old".to_string());
        assert!(set_in(&mut m, "abc", "new", &p).is_err());
        assert_eq!(m.get("abc").map(String::as_str), Some("old"));
        assert!(set_in(&mut m, "xyz", "fresh", &p).is_err());
        assert!(!m.contains_key("xyz"));
        let _ = fs::remove_file(&blocker);
    }
}
