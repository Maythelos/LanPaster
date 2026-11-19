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

/// A device the user removed (either locally or because the peer removed us).
/// It is remembered so automatic discovery does not immediately re-pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokedPeer {
    pub device_id: String,
    pub name: String,
    pub public_key: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustedFile {
    #[serde(default)]
    peers: Vec<TrustedPeer>,
    #[serde(default)]
    revoked: Vec<RevokedPeer>,
}

/// Result of comparing the two sides' trust state after the Hello exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDecision {
    /// Both sides still trust each other.
    Trusted,
    /// Neither side trusts the other: a normal first-time pairing.
    Pair,
    /// We still trust the peer, but it removed us. Remove locally too.
    PeerRemovedUs,
    /// We removed the peer, but it still trusts us. Tell it to remove us.
    WeRemovedPeer,
    /// Neither trusts the other, but we explicitly removed this device before.
    Revoked,
}

pub fn decide_trust(
    local_trusts_remote: bool,
    remote_trusts_me: bool,
    revoked: bool,
) -> TrustDecision {
    match (local_trusts_remote, remote_trusts_me) {
        (true, true) => TrustDecision::Trusted,
        (true, false) => TrustDecision::PeerRemovedUs,
        (false, true) => TrustDecision::WeRemovedPeer,
        (false, false) if revoked => TrustDecision::Revoked,
        (false, false) => TrustDecision::Pair,
    }
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
    revoked: Vec<RevokedPeer>,
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
        let (trusted, revoked) = load_store(&path).unwrap_or_else(|err| {
            warn!("could not read trusted peers {}: {err:#}", path.display());
            (Vec::new(), Vec::new())
        });
        Ok(Self {
            path,
            inner: Mutex::new(Inner {
                trusted,
                revoked,
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
        let trusted = match load_store(&self.path) {
            Ok((trusted, revoked)) => {
                inner.revoked = revoked;
                trusted
            }
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

    /// Same as [`Self::is_trusted`] but only matches the identity key. Used for
    /// the Hello exchange, where the remote device id is not known yet.
    pub fn is_trusted_key(&self, public_key: &str) -> bool {
        self.reload_if_changed();
        let inner = self.inner.lock().unwrap();
        inner
            .trusted
            .iter()
            .any(|p| p.public_key.eq_ignore_ascii_case(public_key))
    }

    pub fn is_revoked(&self, device_id: &str, public_key: &str) -> bool {
        self.reload_if_changed();
        let inner = self.inner.lock().unwrap();
        inner
            .revoked
            .iter()
            .any(|p| p.device_id == device_id || p.public_key.eq_ignore_ascii_case(public_key))
    }

    pub fn trust(&self, device_id: &str, name: &str, public_key: &str) -> Result<()> {
        self.reload_if_changed();
        let mut inner = self.inner.lock().unwrap();
        inner
            .revoked
            .retain(|p| p.device_id != device_id && !p.public_key.eq_ignore_ascii_case(public_key));
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
        save_store(&self.path, &inner.trusted, &inner.revoked)?;
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

    pub fn revoked_list(&self) -> Vec<RevokedPeer> {
        self.reload_if_changed();
        self.inner.lock().unwrap().revoked.clone()
    }

    /// Forget a trusted device by its device id and remember it as removed, so
    /// mDNS discovery will not immediately ask both users to pair again.
    /// Returns true when a device was actually removed.
    pub fn untrust(&self, device_id: &str) -> Result<bool> {
        self.reload_if_changed();
        let mut inner = self.inner.lock().unwrap();
        let removed: Vec<TrustedPeer> = inner
            .trusted
            .iter()
            .filter(|p| p.device_id == device_id)
            .cloned()
            .collect();
        if removed.is_empty() {
            return Ok(false);
        }
        inner.trusted.retain(|p| p.device_id != device_id);
        for peer in &removed {
            push_revoked(&mut inner.revoked, &peer.device_id, &peer.name, &peer.public_key);
        }
        save_store(&self.path, &inner.trusted, &inner.revoked)?;
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
        let removed: Vec<TrustedPeer> = inner.trusted.drain(..).collect();
        for peer in &removed {
            push_revoked(&mut inner.revoked, &peer.device_id, &peer.name, &peer.public_key);
        }
        save_store(&self.path, &inner.trusted, &inner.revoked)?;
        inner.trusted_mtime = Self::trusted_mtime(&self.path);
        Ok(count)
    }

    /// Remove trust for a device and remember it as removed. Used both for a
    /// local removal and when the peer tells us it removed this device.
    pub fn revoke(&self, device_id: &str, name: &str, public_key: &str) -> Result<()> {
        debug!("revoke {device_id} ({name})");
        self.reload_if_changed();
        let mut inner = self.inner.lock().unwrap();
        let before_trusted = inner.trusted.len();
        inner.trusted.retain(|p| p.device_id != device_id);
        let revoked_changed = push_revoked(&mut inner.revoked, device_id, name, public_key);
        if inner.trusted.len() == before_trusted && !revoked_changed {
            return Ok(());
        }
        save_store(&self.path, &inner.trusted, &inner.revoked)?;
        inner.trusted_mtime = Self::trusted_mtime(&self.path);
        Ok(())
    }

    /// Handle an `Unpair` notice from the peer: drop the trust record and only
    /// keep a removal record when this side actually had something to remove
    /// (or already blocked the device). A peer that has already been re-enabled
    /// locally is not blocked again just because the other side is not ready.
    pub fn note_unpaired(&self, device_id: &str, name: &str, public_key: &str) -> Result<()> {
        debug!("note_unpaired {device_id} ({name})");
        self.reload_if_changed();
        let mut inner = self.inner.lock().unwrap();
        let was_trusted = inner.trusted.iter().any(|p| p.device_id == device_id);
        let was_revoked = inner.revoked.iter().any(|p| p.device_id == device_id);
        if !was_trusted && !was_revoked {
            return Ok(());
        }
        inner.trusted.retain(|p| p.device_id != device_id);
        push_revoked(&mut inner.revoked, device_id, name, public_key);
        save_store(&self.path, &inner.trusted, &inner.revoked)?;
        inner.trusted_mtime = Self::trusted_mtime(&self.path);
        Ok(())
    }

    /// Allow a previously removed device to request pairing again.
    pub fn allow(&self, device_id: &str) -> Result<bool> {
        self.reload_if_changed();
        let mut inner = self.inner.lock().unwrap();
        let before = inner.revoked.len();
        inner.revoked.retain(|p| p.device_id != device_id);
        if inner.revoked.len() == before {
            return Ok(false);
        }
        save_store(&self.path, &inner.trusted, &inner.revoked)?;
        inner.trusted_mtime = Self::trusted_mtime(&self.path);
        Ok(true)
    }

    /// Clear every removed-device record.
    pub fn allow_all(&self) -> Result<usize> {
        self.reload_if_changed();
        let mut inner = self.inner.lock().unwrap();
        let count = inner.revoked.len();
        if count == 0 {
            return Ok(0);
        }
        inner.revoked.clear();
        save_store(&self.path, &inner.trusted, &inner.revoked)?;
        inner.trusted_mtime = Self::trusted_mtime(&self.path);
        Ok(count)
    }
}

fn push_revoked(
    revoked: &mut Vec<RevokedPeer>,
    device_id: &str,
    name: &str,
    public_key: &str,
) -> bool {
    if let Some(peer) = revoked.iter_mut().find(|p| p.device_id == device_id) {
        let changed = peer.name != name || !peer.public_key.eq_ignore_ascii_case(public_key);
        peer.name = name.to_string();
        peer.public_key = public_key.to_string();
        changed
    } else {
        revoked.push(RevokedPeer {
            device_id: device_id.to_string(),
            name: name.to_string(),
            public_key: public_key.to_string(),
        });
        true
    }
}

fn load_store(path: &Path) -> Result<(Vec<TrustedPeer>, Vec<RevokedPeer>)> {
    if !path.exists() {
        return Ok((Vec::new(), Vec::new()));
    }
    let raw = fs::read_to_string(path)?;
    let file: TrustedFile = toml::from_str(&raw)?;
    Ok((file.peers, file.revoked))
}

fn save_store(path: &Path, peers: &[TrustedPeer], revoked: &[RevokedPeer]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(&TrustedFile {
        peers: peers.to_vec(),
        revoked: revoked.to_vec(),
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
        assert_eq!(fresh.revoked_list().len(), 1);
        assert!(fresh.is_revoked("dev1", &"aa".repeat(32)));
        assert_eq!(fresh.untrust_all().unwrap(), 1);
        assert!(fresh.trusted_list().is_empty());
        assert_eq!(fresh.revoked_list().len(), 2);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn removed_device_can_be_allowed_again() {
        let path = std::env::temp_dir().join(format!(
            "lanpaster-pairing-test-{}.toml",
            crate::config::random_hex(4)
        ));
        let mgr = PairingManager::new(&path).unwrap();
        let (key, _) = mgr.request("dev1", "Mac", "aa".repeat(32).as_str());
        mgr.approve(&key);
        assert!(mgr.untrust("dev1").unwrap());
        assert!(mgr.is_revoked("dev1", &"aa".repeat(32)));
        assert!(!mgr.untrust("dev1").unwrap());

        // Explicitly trusting the device again clears the removal record.
        mgr.trust("dev1", "Mac", &"aa".repeat(32)).unwrap();
        assert!(!mgr.is_revoked("dev1", &"aa".repeat(32)));
        assert!(mgr.is_trusted("dev1", &"aa".repeat(32)));

        assert!(mgr.untrust("dev1").unwrap());
        assert_eq!(mgr.revoked_list().len(), 1);
        assert!(mgr.allow("dev1").unwrap());
        assert!(!mgr.is_revoked("dev1", &"aa".repeat(32)));
        assert!(!mgr.allow("dev1").unwrap());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn revoke_removes_trust_and_persists_tombstone() {
        let path = std::env::temp_dir().join(format!(
            "lanpaster-pairing-test-{}.toml",
            crate::config::random_hex(4)
        ));
        let mgr = PairingManager::new(&path).unwrap();
        let (key, _) = mgr.request("dev1", "Mac", "aa".repeat(32).as_str());
        mgr.approve(&key);

        // Simulates receiving an Unpair notification from the peer.
        mgr.note_unpaired("dev1", "Mac", &"aa".repeat(32)).unwrap();
        assert!(!mgr.is_trusted("dev1", &"aa".repeat(32)));
        assert!(mgr.is_revoked("dev1", &"aa".repeat(32)));

        let fresh = PairingManager::new(&path).unwrap();
        assert!(fresh.trusted_list().is_empty());
        assert!(fresh.is_revoked("dev1", &"aa".repeat(32)));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn unpair_notice_does_not_reblock_a_locally_restored_device() {
        let path = std::env::temp_dir().join(format!(
            "lanpaster-pairing-test-{}.toml",
            crate::config::random_hex(4)
        ));
        let mgr = PairingManager::new(&path).unwrap();

        // No trust and no removal record: a stale Unpair is a no-op.
        mgr.note_unpaired("dev1", "Mac", &"aa".repeat(32)).unwrap();
        assert!(mgr.revoked_list().is_empty());

        // Once the device was paired, an Unpair removes trust and blocks it.
        let (key, _) = mgr.request("dev1", "Mac", "aa".repeat(32).as_str());
        mgr.approve(&key);
        mgr.note_unpaired("dev1", "Mac", &"aa".repeat(32)).unwrap();
        assert!(!mgr.is_trusted("dev1", &"aa".repeat(32)));
        assert!(mgr.is_revoked("dev1", &"aa".repeat(32)));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn trust_decision_matrix() {
        assert_eq!(decide_trust(true, true, false), TrustDecision::Trusted);
        assert_eq!(decide_trust(true, false, false), TrustDecision::PeerRemovedUs);
        assert_eq!(decide_trust(false, true, true), TrustDecision::WeRemovedPeer);
        assert_eq!(decide_trust(false, false, true), TrustDecision::Revoked);
        assert_eq!(decide_trust(false, false, false), TrustDecision::Pair);
    }

    #[test]
    fn loads_legacy_trusted_file_without_revoked_field() {
        let path = std::env::temp_dir().join(format!(
            "lanpaster-pairing-test-{}.toml",
            crate::config::random_hex(4)
        ));
        fs::write(
            &path,
            format!(
                "[[peers]]\ndevice_id = \"dev1\"\nname = \"Mac\"\npublic_key = \"{}\"\n",
                "aa".repeat(32)
            ),
        )
        .unwrap();
        let mgr = PairingManager::new(&path).unwrap();
        assert_eq!(mgr.trusted_list().len(), 1);
        assert!(mgr.is_trusted("dev1", &"aa".repeat(32)));
        assert!(mgr.revoked_list().is_empty());
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

        // Simulate the settings window editing the file from another process.
        save_store(&path, &[], &[]).unwrap();
        assert!(!mgr.is_trusted("dev1", &"aa".repeat(32)));
        let _ = fs::remove_file(path);
    }
}
