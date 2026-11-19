use crate::config::cache_dir;
use anyhow::{Context, Result};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::warn;

pub struct InstanceLock {
    _file: File,
}

/// One daemon instance per config file. The lock is released when the
/// process exits or the returned value is dropped.
pub fn acquire(config_path: &Path) -> Result<Option<InstanceLock>> {
    acquire_at(&cache_path(config_path, ".lock"))
}

/// One settings window per config file. The window is a separate process, so
/// without this lock every tray click would open another window. The lock is
/// released when the window process exits.
pub fn acquire_ui(config_path: &Path) -> Result<Option<InstanceLock>> {
    acquire_at(&cache_path(config_path, ".ui.lock"))
}

/// Whether a settings window is already running. The probe does not keep the
/// lock; it only tells the caller whether to open a window or wake the
/// existing one.
pub fn ui_running(config_path: &Path) -> bool {
    match acquire_ui(config_path) {
        Ok(Some(lock)) => {
            drop(lock);
            false
        }
        Ok(None) => true,
        Err(err) => {
            warn!("failed to check settings window lock: {err:#}");
            false
        }
    }
}

/// Ask a running settings window to come to the front and switch to `panel`.
/// The window polls this file, so the timestamp makes repeated clicks on the
/// same panel distinct.
pub fn request_ui(config_path: &Path, panel: &str) -> Result<()> {
    let path = cache_path(config_path, ".ui-request");
    std::fs::create_dir_all(cache_dir()).context("create cache dir")?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    std::fs::write(&path, format!("{panel} {stamp}"))
        .with_context(|| format!("write {}", path.display()))
}

pub fn ui_request_path(config_path: &Path) -> PathBuf {
    cache_path(config_path, ".ui-request")
}

fn acquire_at(path: &Path) -> Result<Option<InstanceLock>> {
    std::fs::create_dir_all(cache_dir()).context("create cache dir")?;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("open lock {}", path.display()))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(InstanceLock { _file: file })),
        Err(_) => Ok(None),
    }
}

fn cache_path(config_path: &Path, suffix: &str) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(config_path.to_string_lossy().as_bytes());
    let hash = hex::encode(&hasher.finalize()[..12]);
    cache_dir().join(format!("lanpaster-{hash}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "lanpaster-instance-{name}-{}-{stamp}.toml",
            std::process::id()
        ))
    }

    #[test]
    fn settings_window_lock_is_exclusive_per_config() {
        let path = test_config("ui");
        let first = acquire_ui(&path).expect("first lock").expect("acquire");
        assert!(acquire_ui(&path).expect("second probe").is_none());
        assert!(ui_running(&path));
        drop(first);
        assert!(!ui_running(&path));

        let other = test_config("other");
        assert!(acquire_ui(&other).expect("other lock").is_some());
    }

    #[test]
    fn ui_request_records_panel_and_timestamp() {
        let path = test_config("request");
        request_ui(&path, "local").expect("request");
        let text = std::fs::read_to_string(ui_request_path(&path)).expect("read request");
        let mut parts = text.split_whitespace();
        assert_eq!(parts.next(), Some("local"));
        assert!(parts
            .next()
            .and_then(|stamp| stamp.parse::<u128>().ok())
            .is_some());

        std::thread::sleep(std::time::Duration::from_millis(2));
        request_ui(&path, "local").expect("second request");
        assert_ne!(
            text,
            std::fs::read_to_string(ui_request_path(&path)).expect("read request")
        );
    }
}
