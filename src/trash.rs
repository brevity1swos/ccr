use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const AUTO_PRUNE_DAYS: u64 = 30;

pub fn trash_root() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("CCR_TRASH_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let home = dirs::home_dir().context("no home dir")?;
    Ok(home.join(".ccr").join("trash"))
}

pub fn move_to_trash(source: &Path, backend: &str, id: &str) -> Result<PathBuf> {
    move_to_trash_in(&trash_root()?, source, backend, id)
}

fn move_to_trash_in(root: &Path, source: &Path, backend: &str, id: &str) -> Result<PathBuf> {
    let dest = root.join(backend).join(format!("{id}.jsonl"));
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create trash dir {}", parent.display()))?;
    }
    fs::rename(source, &dest)
        .with_context(|| format!("move {} -> {}", source.display(), dest.display()))?;
    Ok(dest)
}

/// Delete any trashed files older than `AUTO_PRUNE_DAYS`. Returns the count
/// removed. Missing root is a no-op.
pub fn auto_prune() -> Result<usize> {
    auto_prune_in(&trash_root()?)
}

fn auto_prune_in(root: &Path) -> Result<usize> {
    if !root.exists() {
        return Ok(0);
    }
    let threshold = SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(AUTO_PRUNE_DAYS * 86400))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut removed = 0usize;
    visit_files(root, &mut |path| {
        if let Ok(meta) = fs::metadata(path)
            && let Ok(mtime) = meta.modified()
            && mtime < threshold
            && fs::remove_file(path).is_ok()
        {
            removed += 1;
        }
    })?;
    Ok(removed)
}

fn visit_files(dir: &Path, cb: &mut impl FnMut(&Path)) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if entry.file_type()?.is_dir() {
            visit_files(&p, cb)?;
        } else {
            cb(&p);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn unique_tmp(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "ccr-test-{}-{}-{}-{}",
            label,
            std::process::id(),
            n,
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn move_to_trash_relocates_file() {
        let root = unique_tmp("move");
        let src = root.parent().unwrap().join(format!(
            "ccr-src-{}-{}.jsonl",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        {
            let mut f = fs::File::create(&src).unwrap();
            writeln!(f, "data").unwrap();
        }

        let dest = move_to_trash_in(&root, &src, "claude", "xyz").unwrap();
        assert!(dest.exists());
        assert!(!src.exists());
        assert!(dest.ends_with("claude/xyz.jsonl"));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn move_to_trash_creates_nested_dirs() {
        let root = unique_tmp("nested");
        let src = root.parent().unwrap().join(format!(
            "ccr-src-nested-{}-{}.jsonl",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::File::create(&src).unwrap();

        let dest = move_to_trash_in(&root, &src, "opencode", "abc").unwrap();
        assert!(dest.parent().unwrap().is_dir());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn auto_prune_on_missing_root_is_noop() {
        let missing = std::env::temp_dir().join(format!(
            "ccr-missing-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        assert!(!missing.exists());
        assert_eq!(auto_prune_in(&missing).unwrap(), 0);
    }

    #[test]
    fn auto_prune_leaves_fresh_files_alone() {
        let root = unique_tmp("fresh");
        let sub = root.join("claude");
        fs::create_dir_all(&sub).unwrap();
        let f = sub.join("recent.jsonl");
        fs::File::create(&f).unwrap();

        assert_eq!(auto_prune_in(&root).unwrap(), 0);
        assert!(f.exists());

        fs::remove_dir_all(&root).ok();
    }
}
