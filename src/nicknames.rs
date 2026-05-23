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

pub fn save(nicknames: &HashMap<String, String>) -> Result<()> {
    save_to(nicknames, &nicknames_path()?)
}

fn save_to(nicknames: &HashMap<String, String>, path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_string_pretty(nicknames)?)
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
    if name.trim().is_empty() {
        nicknames.remove(id);
    } else {
        nicknames.insert(id.to_string(), name.trim().to_string());
    }
    save(nicknames)
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
        unsafe { std::env::set_var("CCR_NICKNAMES_FILE", &p) };
        let mut m = HashMap::new();
        set(&mut m, "abc", "hello").unwrap();
        assert_eq!(m.get("abc").map(String::as_str), Some("hello"));
        set(&mut m, "abc", "").unwrap();
        assert!(!m.contains_key("abc"));
        unsafe { std::env::remove_var("CCR_NICKNAMES_FILE") };
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn set_trims_whitespace() {
        let p = tmp_path();
        unsafe { std::env::set_var("CCR_NICKNAMES_FILE", &p) };
        let mut m = HashMap::new();
        set(&mut m, "abc", "  hello  ").unwrap();
        assert_eq!(m.get("abc").map(String::as_str), Some("hello"));
        unsafe { std::env::remove_var("CCR_NICKNAMES_FILE") };
        let _ = fs::remove_file(&p);
    }
}
