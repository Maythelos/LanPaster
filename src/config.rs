use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub device_name: String,
    pub device_id: String,
    /// Persistent X25519 noise identity (32 bytes, hex encoded).
    pub private_key: String,
    pub public_key: String,
    pub listen_port: u16,
    pub poll_interval_ms: u64,
    pub max_text_kb: u64,
    pub max_file_mb: u64,
    pub max_batch_mb: u64,
    pub max_files: usize,
    pub file_sync: bool,
    pub sound_enabled: bool,
    pub cache_max_mb: u64,
    /// How long in-memory clipboard history entries are kept.
    pub history_minutes: u64,
    /// Maximum number of in-memory clipboard history entries.
    pub history_limit: usize,
    pub mdns_enabled: bool,
    /// Manual peers in `host:port` form.
    pub peers: Vec<String>,
    pub log_level: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device_name: hostname::get()
                .ok()
                .and_then(|h| h.into_string().ok())
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| "LanPaster".to_string()),
            device_id: String::new(),
            private_key: String::new(),
            public_key: String::new(),
            listen_port: 39393,
            poll_interval_ms: 500,
            max_text_kb: 1024,
            max_file_mb: 100,
            max_batch_mb: 200,
            max_files: 50,
            file_sync: true,
            sound_enabled: true,
            cache_max_mb: 1024,
            history_minutes: 10,
            history_limit: 20,
            mdns_enabled: true,
            peers: Vec::new(),
            log_level: "info".to_string(),
        }
    }
}

impl Config {
    pub fn load_from(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        toml::from_str::<Self>(&raw)
            .with_context(|| format!("parse config {}", path.display()))
    }

    pub fn load_or_create_at(path: &Path) -> Result<(Self, PathBuf)> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create config dir {}", parent.display()))?;
        }

        let mut cfg = if path.exists() {
            let raw = fs::read_to_string(path)
                .with_context(|| format!("read config {}", path.display()))?;
            toml::from_str::<Self>(&raw)
                .with_context(|| format!("parse config {}", path.display()))?
        } else {
            Self::default()
        };

        let mut changed = false;
        if cfg.device_id.trim().is_empty() {
            cfg.device_id = random_hex(8);
            changed = true;
        }
        if cfg.private_key.trim().is_empty() || cfg.public_key.trim().is_empty() {
            let (private, public) = crate::crypto::generate_identity();
            cfg.private_key = private;
            cfg.public_key = public;
            changed = true;
        }
        if cfg.device_name.trim().is_empty() {
            cfg.device_name = Self::default().device_name;
            changed = true;
        }
        if cfg.listen_port == 0 {
            cfg.listen_port = Self::default().listen_port;
            changed = true;
        }

        if changed || !path.exists() {
            cfg.save(path)?;
        }
        Ok((cfg, path.to_path_buf()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                f.set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn private_key_bytes(&self) -> Result<Vec<u8>> {
        decode_key(&self.private_key, "private_key")
    }

    pub fn public_key_bytes(&self) -> Result<Vec<u8>> {
        decode_key(&self.public_key, "public_key")
    }

    pub fn validate(&self) -> Result<()> {
        if self.listen_port == 0 {
            bail!("listen_port must not be 0");
        }
        self.private_key_bytes()?;
        self.public_key_bytes()?;
        if self.max_text_kb == 0
            || self.max_file_mb == 0
            || self.max_batch_mb == 0
            || self.max_files == 0
        {
            bail!("size limits must be greater than zero");
        }
        Ok(())
    }

    pub fn max_text_bytes(&self) -> u64 {
        self.max_text_kb.saturating_mul(1024)
    }

    pub fn max_file_bytes(&self) -> u64 {
        self.max_file_mb.saturating_mul(1024 * 1024)
    }

    pub fn max_batch_bytes(&self) -> u64 {
        self.max_batch_mb.saturating_mul(1024 * 1024)
    }
}

fn decode_key(value: &str, name: &str) -> Result<Vec<u8>> {
    let bytes = hex::decode(value.trim()).with_context(|| format!("{name} must be hex"))?;
    if bytes.len() != 32 {
        bail!("{name} must be 32 bytes (64 hex chars), got {}", bytes.len());
    }
    Ok(bytes)
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("LanPaster")
        .join("config.toml")
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| std::env::temp_dir())
        .join("LanPaster")
}

pub fn incoming_dir() -> PathBuf {
    cache_dir().join("incoming")
}

pub fn random_hex(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    getrandom::getrandom(&mut bytes).expect("OS random source");
    hex::encode(bytes)
}

pub fn random_u64() -> u64 {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).expect("OS random source");
    u64::from_le_bytes(bytes).max(1)
}
