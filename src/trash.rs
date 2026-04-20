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

pub fn trash_path(backend: &str, id: &str) -> Result<PathBuf> {
    Ok(trash_root()?.join(backend).join(format!("{id}.jsonl")))
}

pub fn move_to_trash(source: &Path, backend: &str, id: &str) -> Result<PathBuf> {
    let dest = trash_path(backend, id)?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create trash dir {}", parent.display()))?;
    }
    fs::rename(source, &dest)
        .with_context(|| format!("move {} -> {}", source.display(), dest.display()))?;
    Ok(dest)
}

/// Delete any trashed files older than `AUTO_PRUNE_DAYS`. Returns the count
/// removed. Errors are swallowed per-file (best-effort cleanup on launch).
pub fn auto_prune() -> Result<usize> {
    let root = trash_root()?;
    if !root.exists() {
        return Ok(0);
    }
    let threshold = SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(AUTO_PRUNE_DAYS * 86400))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut removed = 0usize;
    visit_files(&root, &mut |path| {
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

    fn with_temp_trash<F: FnOnce(&Path)>(f: F) {
        let dir = std::env::temp_dir().join(format!("ccr-trash-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // SAFETY: single-threaded test.
        unsafe {
            std::env::set_var("CCR_TRASH_DIR", &dir);
        }
        f(&dir);
        unsafe {
            std::env::remove_var("CCR_TRASH_DIR");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn move_to_trash_relocates_file() {
        with_temp_trash(|_| {
            let src = std::env::temp_dir().join(format!("ccr-src-{}.jsonl", std::process::id()));
            {
                let mut f = fs::File::create(&src).unwrap();
                writeln!(f, "data").unwrap();
            }
            let dest = move_to_trash(&src, "claude", "xyz").unwrap();
            assert!(dest.exists());
            assert!(!src.exists());
            assert!(dest.ends_with("claude/xyz.jsonl"));
        });
    }

    #[test]
    fn trash_path_uses_backend_and_id() {
        with_temp_trash(|root| {
            let p = trash_path("claude", "abc-123").unwrap();
            assert!(p.starts_with(root));
            assert!(p.ends_with("claude/abc-123.jsonl"));
        });
    }

    #[test]
    fn auto_prune_on_missing_root_is_noop() {
        with_temp_trash(|root| {
            fs::remove_dir_all(root).unwrap();
            assert_eq!(auto_prune().unwrap(), 0);
        });
    }
}
