use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const AUTO_PRUNE_DAYS: u64 = 30;
const META_EXT: &str = "meta.json";

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
    let backend_dir = root.join(backend);
    fs::create_dir_all(&backend_dir)
        .with_context(|| format!("create trash dir {}", backend_dir.display()))?;
    let dest_data = backend_dir.join(format!("{id}.jsonl"));
    let dest_meta = backend_dir.join(format!("{id}.{META_EXT}"));
    fs::rename(source, &dest_data)
        .with_context(|| format!("move {} -> {}", source.display(), dest_data.display()))?;
    let meta = serde_json::json!({ "origin": source.to_string_lossy() });
    fs::write(&dest_meta, meta.to_string())
        .with_context(|| format!("write sidecar {}", dest_meta.display()))?;
    Ok(dest_data)
}

/// A session currently sitting in the ccr trash, with enough info to restore it.
#[derive(Debug, Clone)]
pub struct TrashedItem {
    pub backend: String,
    pub id: String,
    pub origin: PathBuf,
    pub trash_path: PathBuf,
    pub trashed_at: SystemTime,
}

pub fn list_trashed() -> Result<Vec<TrashedItem>> {
    list_trashed_in(&trash_root()?)
}

fn list_trashed_in(root: &Path) -> Result<Vec<TrashedItem>> {
    let mut items = Vec::new();
    if !root.exists() {
        return Ok(items);
    }
    for backend_dir in fs::read_dir(root)? {
        let backend_dir = backend_dir?;
        if !backend_dir.file_type()?.is_dir() {
            continue;
        }
        let backend = backend_dir.file_name().to_string_lossy().into_owned();
        for entry in fs::read_dir(backend_dir.path())? {
            let entry = entry?;
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(id) = p.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let meta_path = p
                .parent()
                .unwrap_or(Path::new("."))
                .join(format!("{id}.{META_EXT}"));
            let origin = fs::read_to_string(&meta_path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v.get("origin").and_then(|o| o.as_str()).map(PathBuf::from))
                .unwrap_or_default();
            let trashed_at = fs::metadata(&p)
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            items.push(TrashedItem {
                backend: backend.clone(),
                id: id.to_string(),
                origin,
                trash_path: p,
                trashed_at,
            });
        }
    }
    items.sort_by(|a, b| b.trashed_at.cmp(&a.trashed_at));
    Ok(items)
}

pub fn restore(item: &TrashedItem) -> Result<()> {
    if item.origin.as_os_str().is_empty() {
        anyhow::bail!(
            "no origin recorded for {} — sidecar missing or unreadable",
            item.id
        );
    }
    if item.origin.exists() {
        anyhow::bail!(
            "refusing to overwrite — {} already exists",
            item.origin.display()
        );
    }
    if let Some(parent) = item.origin.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create origin dir {}", parent.display()))?;
    }
    fs::rename(&item.trash_path, &item.origin).with_context(|| {
        format!(
            "restore {} -> {}",
            item.trash_path.display(),
            item.origin.display()
        )
    })?;
    let meta = item
        .trash_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!("{}.{META_EXT}", item.id));
    let _ = fs::remove_file(meta);
    Ok(())
}

/// Delete trashed files older than `AUTO_PRUNE_DAYS`. Missing root is a no-op.
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

    fn make_src(n: usize) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "ccr-src-{}-{}.jsonl",
            std::process::id(),
            COUNTER.fetch_add(n, Ordering::SeqCst)
        ));
        writeln!(fs::File::create(&p).unwrap(), "data").unwrap();
        p
    }

    #[test]
    fn move_to_trash_writes_sidecar() {
        let root = unique_tmp("sidecar");
        let src = make_src(1);
        let src_copy = src.clone();

        move_to_trash_in(&root, &src, "claude", "xyz").unwrap();
        let meta = root.join("claude").join(format!("xyz.{META_EXT}"));
        assert!(meta.exists());
        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&meta).unwrap()).unwrap();
        assert_eq!(
            content["origin"].as_str().unwrap(),
            src_copy.to_string_lossy()
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn list_trashed_returns_items_sorted_by_mtime_desc() {
        let root = unique_tmp("list");
        move_to_trash_in(&root, &make_src(10), "claude", "first").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        move_to_trash_in(&root, &make_src(11), "codex", "second").unwrap();

        let items = list_trashed_in(&root).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "second");
        assert_eq!(items[1].id, "first");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn restore_round_trip() {
        let root = unique_tmp("roundtrip");
        let src = make_src(20);
        let src_copy = src.clone();
        move_to_trash_in(&root, &src, "claude", "abc").unwrap();
        assert!(!src_copy.exists(), "source should be moved away");

        let items = list_trashed_in(&root).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].origin, src_copy);

        restore(&items[0]).unwrap();
        assert!(src_copy.exists(), "restored file should exist at origin");
        // meta cleaned up
        assert!(!root.join("claude").join(format!("abc.{META_EXT}")).exists());

        let _ = fs::remove_file(&src_copy);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn restore_refuses_to_overwrite_existing_file() {
        let root = unique_tmp("noclobber");
        let src = make_src(30);
        let src_copy = src.clone();
        move_to_trash_in(&root, &src, "claude", "dup").unwrap();

        // Re-create a file at the origin path to simulate conflict
        fs::File::create(&src_copy).unwrap();

        let items = list_trashed_in(&root).unwrap();
        let err = restore(&items[0]).unwrap_err().to_string();
        assert!(err.contains("refusing to overwrite"));

        let _ = fs::remove_file(&src_copy);
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
        move_to_trash_in(&root, &make_src(40), "claude", "recent").unwrap();
        assert_eq!(auto_prune_in(&root).unwrap(), 0);
        fs::remove_dir_all(&root).ok();
    }
}
