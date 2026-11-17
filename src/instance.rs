use crate::config::cache_dir;
use anyhow::{Context, Result};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    path::Path,
};

pub struct InstanceLock {
    _file: File,
}

/// One daemon instance per config file. The lock is released when the
/// process exits or the returned value is dropped.
pub fn acquire(config_path: &Path) -> Result<Option<InstanceLock>> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).context("create cache dir")?;
    let mut hasher = Sha256::new();
    hasher.update(config_path.to_string_lossy().as_bytes());
    let hash = hex::encode(&hasher.finalize()[..12]);
    let path = dir.join(format!("lanpaster-{hash}.lock"));
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("open lock {}", path.display()))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(InstanceLock { _file: file })),
        Err(_) => Ok(None),
    }
}
