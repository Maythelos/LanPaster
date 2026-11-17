use crate::config::incoming_dir;
use std::{
    fs,
    path::Path,
    time::UNIX_EPOCH,
};
use tracing::{debug, warn};

pub fn cleanup_incoming(keep: Option<&Path>, max_bytes: u64) {
    let root = incoming_dir();
    cleanup_dir(&root, keep, max_bytes);
}

pub fn cleanup_dir(root: &Path, keep: Option<&Path>, max_bytes: u64) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    let mut dirs = Vec::new();
    let mut total = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let size = dir_size(&path);
        let modified = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(UNIX_EPOCH);
        total = total.saturating_add(size);
        dirs.push((path, modified, size));
    }

    if total <= max_bytes {
        return;
    }

    dirs.sort_by_key(|(_, modified, _)| *modified);
    for (path, _, size) in dirs {
        if total <= max_bytes {
            break;
        }
        if keep.map(|k| k == path).unwrap_or(false) {
            continue;
        }
        match fs::remove_dir_all(&path) {
            Ok(()) => {
                total = total.saturating_sub(size);
                debug!("removed old incoming cache {}", path.display());
            }
            Err(err) => warn!("failed to remove {}: {err}", path.display()),
        }
    }
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return 0,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total = total.saturating_add(dir_size(&path));
        } else if let Ok(meta) = entry.metadata() {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_keeps_newest_and_respects_limit() {
        let root = std::env::temp_dir().join(format!(
            "lanpaster-cache-test-{}",
            crate::config::random_hex(4)
        ));
        let old = root.join("old");
        let new = root.join("new");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(old.join("a"), vec![0u8; 800]).unwrap();
        fs::write(new.join("b"), vec![0u8; 800]).unwrap();
        cleanup_dir(&root, Some(&new), 1000);
        assert!(!old.exists());
        assert!(new.exists());
        let _ = fs::remove_dir_all(&root);
    }
}
