use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedPeer {
    pub device_id: String,
    pub name: String,
    pub public_key: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustedFile {
    #[serde(default)]
    peers: Vec<TrustedPeer>,
}

#[derive(Debug, Clone)]
pub struct PendingInfo {
    pub key: String,
    pub name: String,
    pub fingerprint: String,
}

struct Pending {
    device_id: String,
    name: String,
    public_key: String,
    fingerprint: String,
    tx: broadcast::Sender<bool>,
}

struct Inner {
    trusted: Vec<TrustedPeer>,
    pending: std::collections::HashMap<String, Pending>,
    trusted_mtime: Option<SystemTime>,
}

pub struct PairingManager {
    path: PathBuf,
    inner: Mutex<Inner>,
}

impl PairingManager {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let trusted_mtime = Self::trusted_mtime(&path);
        let trusted = load_trusted(&path).unwrap_or_else(|err| {
            warn!("could not read trusted peers {}: {err:#}", path.display());
            Vec::new()
        });
        Ok(Self {
            path,
            inner: Mutex::new(Inner {
                trusted,
                pending: std::collections::HashMap::new(),
                trusted_mtime,
            }),
        })
    }

    fn trusted_mtime(path: &Path) -> Option<SystemTime> {
        fs::metadata(path).ok()?.modified().ok()
    }

    /// Pick up changes made by the `lanpaster forget` CLI while the daemon is
    /// running. The trusted file is small, so checking its mtime on each
    /// access is cheap and keeps pairing decisions in sync with the file.
    fn reload_if_changed(&self) {
        let current = Self::trusted_mtime(&self.path);
        let mut inner = self.inner.lock().unwrap();
        if current == inner.trusted_mtime {
            return;
        }
        let trusted = match load_trusted(&self.path) {
            Ok(trusted) => trusted,
            Err(err) => {
                warn!(
                    "could not reload trusted peers {}: {err:#}",
                    self.path.display()
                );
                return;
            }
        };
        inner.trusted = trusted;
        inner.trusted_mtime = current;
    }

    pub fn fingerprint(public_key: &[u8]) -> String {
        let digest = Sha256::digest(public_key);
        hex::encode(&digest[..8]).to_ascii_uppercase()
    }

    pub fn is_trusted(&self, device_id: &str, public_key: &str) -> bool {
        self.reload_if_changed();
        let inner = self.inner.lock().unwrap();
        inner
            .trusted
            .iter()
            .any(|p| p.device_id == device_id && p.public_key.eq_ignore_ascii_case(public_key))
    }

    pub fn trust(&self, device_id: &str, name: &str, public_key: &str) -> Result<()> {
        self.reload_if_changed();
        let mut inner = self.inner.lock().unwrap();
        if let Some(peer) = inner.trusted.iter_mut().find(|p| p.device_id == device_id) {
            peer.name = name.to_string();
            peer.public_key = public_key.to_string();
        } else {
            inner.trusted.push(TrustedPeer {
                device_id: device_id.to_string(),
                name: name.to_string(),
                public_key: public_key.to_string(),
            });
        }
        save_trusted(&self.path, &inner.trusted)?;
        inner.trusted_mtime = Self::trusted_mtime(&self.path);
        Ok(())
    }

    /// Register a pending request, or subscribe to an existing one. The returned
    /// key must be used for approve/deny/cancel.
    pub fn request(
        &self,
        device_id: &str,
        name: &str,
        public_key: &str,
    ) -> (String, broadcast::Receiver<bool>) {
        let fingerprint = Self::fingerprint(
            &hex::decode(public_key).unwrap_or_default(),
        );
        let key = format!("{device_id}:{fingerprint}");
        let mut inner = self.inner.lock().unwrap();
        if let Some(pending) = inner.pending.get(&key) {
            return (key, pending.tx.subscribe());
        }
        let (tx, rx) = broadcast::channel(4);
        inner.pending.insert(
            key.clone(),
            Pending {
                device_id: device_id.to_string(),
                name: name.to_string(),
                public_key: public_key.to_string(),
                fingerprint: fingerprint.clone(),
                tx,
            },
        );
        info!("pairing request from {name} ({fingerprint})");
        (key, rx)
    }

    pub fn approve(&self, key: &str) -> bool {
        let pending = {
            let mut inner = self.inner.lock().unwrap();
            inner.pending.remove(key)
        };
        let Some(pending) = pending else {
            return false;
        };
        if let Err(err) = self.trust(&pending.device_id, &pending.name, &pending.public_key) {
            warn!("failed to save trusted peer: {err:#}");
        }
        let _ = pending.tx.send(true);
        true
    }

    pub fn deny(&self, key: &str) -> bool {
        let pending = {
            let mut inner = self.inner.lock().unwrap();
            inner.pending.remove(key)
        };
        if let Some(pending) = pending {
            let _ = pending.tx.send(false);
            true
        } else {
            false
        }
    }

    pub fn cancel(&self, key: &str) {
        if self.deny(key) {
            debug!("cancelled pairing request {key}");
        }
    }

    pub fn pending_list(&self) -> Vec<PendingInfo> {
        let inner = self.inner.lock().unwrap();
        inner
            .pending
            .iter()
            .map(|(key, p)| PendingInfo {
                key: key.clone(),
                name: p.name.clone(),
                fingerprint: p.fingerprint.clone(),
            })
            .collect()
    }

    pub fn trusted_list(&self) -> Vec<TrustedPeer> {
        self.reload_if_changed();
        self.inner.lock().unwrap().trusted.clone()
    }

    /// Forget a trusted device by its device id. Returns true when a device
    /// was actually removed. The running daemon picks up the change through
    /// [`Self::reload_if_changed`] without needing a restart.
    pub fn untrust(&self, device_id: &str) -> Result<bool> {
        self.reload_if_changed();
        let mut inner = self.inner.lock().unwrap();
        let remaining: Vec<_> = inner
            .trusted
            .iter()
            .filter(|p| p.device_id != device_id)
            .cloned()
            .collect();
        if remaining.len() == inner.trusted.len() {
            return Ok(false);
        }
        save_trusted(&self.path, &remaining)?;
        inner.trusted = remaining;
        inner.trusted_mtime = Self::trusted_mtime(&self.path);
        Ok(true)
    }

    /// Forget every trusted device. Returns the number of removed devices.
    pub fn untrust_all(&self) -> Result<usize> {
        self.reload_if_changed();
        let mut inner = self.inner.lock().unwrap();
        let count = inner.trusted.len();
        if count == 0 {
            return Ok(0);
        }
        save_trusted(&self.path, &[])?;
        inner.trusted.clear();
        inner.trusted_mtime = Self::trusted_mtime(&self.path);
        Ok(count)
    }
}

fn load_trusted(path: &Path) -> Result<Vec<TrustedPeer>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path)?;
    let file: TrustedFile = toml::from_str(&raw)?;
    Ok(file.peers)
}

fn save_trusted(path: &Path, peers: &[TrustedPeer]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(&TrustedFile {
        peers: peers.to_vec(),
    })?;
    let tmp = path.with_extension("toml.tmp");
    {
        let mut file = fs::File::create(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path).with_context(|| format!("save {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approve_persists_trusted_peer() {
        let path = std::env::temp_dir().join(format!(
            "lanpaster-pairing-test-{}.toml",
            crate::config::random_hex(4)
        ));
        let mgr = PairingManager::new(&path).unwrap();
        let (key, _rx) = mgr.request("dev1", "Mac", "aa".repeat(32).as_str());
        assert_eq!(mgr.pending_list().len(), 1);
        assert!(mgr.approve(&key));
        assert!(mgr.is_trusted("dev1", &"aa".repeat(32)));
        assert!(path.exists());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn untrust_removes_and_persists() {
        let path = std::env::temp_dir().join(format!(
            "lanpaster-pairing-test-{}.toml",
            crate::config::random_hex(4)
        ));
        let mgr = PairingManager::new(&path).unwrap();
        let (key, _) = mgr.request("dev1", "Mac", "aa".repeat(32).as_str());
        mgr.approve(&key);
        let (key, _) = mgr.request("dev2", "PC", "bb".repeat(32).as_str());
        mgr.approve(&key);
        assert_eq!(mgr.trusted_list().len(), 2);

        assert!(mgr.untrust("dev1").unwrap());
        assert_eq!(mgr.trusted_list().len(), 1);
        assert!(!mgr.is_trusted("dev1", &"aa".repeat(32)));
        assert!(mgr.is_trusted("dev2", &"bb".repeat(32)));
        assert!(!mgr.untrust("dev1").unwrap());

        // A fresh manager sees the persisted result.
        let fresh = PairingManager::new(&path).unwrap();
        assert_eq!(fresh.trusted_list().len(), 1);
        assert_eq!(fresh.untrust_all().unwrap(), 1);
        assert!(fresh.trusted_list().is_empty());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn detects_external_trusted_file_change() {
        let path = std::env::temp_dir().join(format!(
            "lanpaster-pairing-test-{}.toml",
            crate::config::random_hex(4)
        ));
        let mgr = PairingManager::new(&path).unwrap();
        let (key, _) = mgr.request("dev1", "Mac", "aa".repeat(32).as_str());
        mgr.approve(&key);
        assert!(mgr.is_trusted("dev1", &"aa".repeat(32)));

        // Simulate `lanpaster forget dev1` editing the file from another process.
        save_trusted(&path, &[]).unwrap();
        assert!(!mgr.is_trusted("dev1", &"aa".repeat(32)));
        let _ = fs::remove_file(path);
    }
}
