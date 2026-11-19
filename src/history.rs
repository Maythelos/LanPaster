use crate::clipboard::RemoteClipboard;
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tracing::debug;

#[derive(Debug, Clone)]
pub enum HistoryPayload {
    Text(String),
    Image {
        mime: String,
        bytes: Arc<Vec<u8>>,
        width: Option<u32>,
        height: Option<u32>,
    },
    Files {
        paths: Vec<PathBuf>,
        total_size: u64,
    },
}

#[derive(Debug, Clone)]
pub struct HistoryItem {
    pub id: u64,
    pub recorded_at: Instant,
    pub origin: String,
    fingerprint: Vec<u8>,
    pub payload: HistoryPayload,
}

impl HistoryItem {
    pub fn preview(&self) -> String {
        match &self.payload {
            HistoryPayload::Text(text) => {
                let sanitized: String = text
                    .chars()
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .collect();
                let compact = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
                if compact.is_empty() {
                    "(空文本)".to_string()
                } else {
                    truncate_chars(&compact, 48)
                }
            }
            HistoryPayload::Image {
                bytes,
                width,
                height,
                ..
            } => {
                let dims = match (width, height) {
                    (Some(w), Some(h)) => format!(" {w}×{h}"),
                    _ => String::new(),
                };
                format!("图片{dims} · {}", format_size(bytes.len() as u64))
            }
            HistoryPayload::Files { paths, total_size } => {
                format!("{} 个文件 · {}", paths.len(), format_size(*total_size))
            }
        }
    }

    pub fn is_restorable(&self) -> bool {
        match &self.payload {
            HistoryPayload::Files { paths, .. } => paths.iter().any(|p| p.is_file()),
            _ => true,
        }
    }

    /// History restore creates an owned clipboard payload. Images/text are
    /// stored once in memory and cloned only when the user picks an entry.
    pub fn to_remote(&self) -> Option<RemoteClipboard> {
        match &self.payload {
            HistoryPayload::Text(text) => Some(RemoteClipboard::Text(text.clone())),
            HistoryPayload::Image { mime, bytes, .. } => Some(RemoteClipboard::Image {
                mime: mime.clone(),
                bytes: bytes.as_ref().clone(),
            }),
            HistoryPayload::Files { paths, .. } => {
                let existing: Vec<PathBuf> = paths
                    .iter()
                    .filter(|path| path.is_file())
                    .cloned()
                    .collect();
                if existing.is_empty() {
                    None
                } else {
                    Some(RemoteClipboard::Files(existing))
                }
            }
        }
    }
}

struct Inner {
    items: VecDeque<Arc<HistoryItem>>,
    next_id: u64,
}

impl Inner {
    fn prune(&mut self, minutes: u64, limit: usize) {
        if minutes == 0 || limit == 0 {
            self.items.clear();
            return;
        }
        let max_age = Duration::from_secs(minutes.saturating_mul(60));
        let now = Instant::now();
        self.items
            .retain(|item| now.saturating_duration_since(item.recorded_at) <= max_age);
        self.items.truncate(limit);
    }
}

pub struct History {
    inner: Mutex<Inner>,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                items: VecDeque::new(),
                next_id: 1,
            }),
        }
    }

    pub fn record_remote(
        &self,
        payload: RemoteClipboard,
        origin: &str,
        minutes: u64,
        limit: usize,
    ) {
        let payload = match payload {
            RemoteClipboard::Text(text) => HistoryPayload::Text(text),
            RemoteClipboard::Image { mime, bytes } => HistoryPayload::Image {
                mime,
                bytes: Arc::new(bytes),
                width: None,
                height: None,
            },
            RemoteClipboard::Files(paths) => {
                let total_size = paths.iter().map(|path| file_size(path)).sum();
                HistoryPayload::Files { paths, total_size }
            }
        };
        self.record(payload, origin, minutes, limit);
    }

    pub fn record(
        &self,
        payload: HistoryPayload,
        origin: &str,
        minutes: u64,
        limit: usize,
    ) {
        self.insert(payload, origin, Instant::now(), minutes, limit);
    }

    fn insert(
        &self,
        payload: HistoryPayload,
        origin: &str,
        recorded_at: Instant,
        minutes: u64,
        limit: usize,
    ) {
        if minutes == 0 || limit == 0 {
            self.clear();
            return;
        }
        let fingerprint = payload.fingerprint();
        let mut inner = self.inner.lock().unwrap();
        inner.prune(minutes, limit);
        inner.items.retain(|item| item.fingerprint != fingerprint);
        let id = inner.next_id;
        inner.next_id = inner.next_id.saturating_add(1);
        inner.items.push_front(Arc::new(HistoryItem {
            id,
            recorded_at,
            origin: origin.to_string(),
            fingerprint,
            payload,
        }));
        inner.items.truncate(limit);
        debug!(
            "history recorded from {origin}: {} item(s), limit {}",
            inner.items.len(),
            limit
        );
    }

    pub fn snapshot(&self, minutes: u64, limit: usize) -> Vec<Arc<HistoryItem>> {
        let mut inner = self.inner.lock().unwrap();
        inner.prune(minutes, limit);
        inner.items.iter().cloned().collect()
    }

    pub fn prune(&self, minutes: u64, limit: usize) {
        self.inner.lock().unwrap().prune(minutes, limit);
    }

    pub fn clear(&self) {
        self.inner.lock().unwrap().items.clear();
    }
}

impl HistoryPayload {
    fn fingerprint(&self) -> Vec<u8> {
        let mut hasher = Sha256::new();
        match self {
            HistoryPayload::Text(text) => {
                hasher.update(b"text\0");
                hasher.update(text.as_bytes());
            }
            HistoryPayload::Image { mime, bytes, .. } => {
                hasher.update(b"image\0");
                hasher.update(mime.as_bytes());
                hasher.update(b"\0");
                hasher.update(bytes.as_slice());
            }
            HistoryPayload::Files { paths, .. } => {
                hasher.update(b"files\0");
                for path in paths {
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    hasher.update(name.as_bytes());
                    hasher.update(b"\0");
                    hasher.update(file_size(path).to_le_bytes());
                }
            }
        }
        hasher.finalize().to_vec()
    }
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn truncate_chars(text: &str, max: usize) -> String {
    let mut truncated: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        truncated.push('…');
    }
    truncated
}

fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    if bytes as f64 >= MB {
        format!("{:.1} MB", bytes as f64 / MB)
    } else if bytes as f64 >= KB {
        format!("{:.1} KB", bytes as f64 / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> HistoryPayload {
        HistoryPayload::Text(value.to_string())
    }

    #[test]
    fn keeps_newest_items_and_dedupes() {
        let history = History::new();
        history.record(text("one"), "本机", 10, 2);
        history.record(text("two"), "本机", 10, 2);
        history.record(text("three"), "本机", 10, 2);
        let items = history.snapshot(10, 2);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].preview(), "three");
        assert_eq!(items[1].preview(), "two");

        // Recording the same content moves it to the front instead of adding.
        history.record(text("two"), "对端", 10, 2);
        let items = history.snapshot(10, 2);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].preview(), "two");
        assert_eq!(items[0].origin, "对端");
    }

    #[test]
    fn remote_payloads_keep_their_origin() {
        let history = History::new();
        history.record_remote(
            RemoteClipboard::Text("remote".to_string()),
            "Linux",
            10,
            10,
        );
        let items = history.snapshot(10, 10);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].origin, "Linux");
        assert_eq!(items[0].preview(), "remote");
    }

    #[test]
    fn expires_items_older_than_the_window() {
        let history = History::new();
        history.insert(
            text("old"),
            "本机",
            Instant::now() - Duration::from_secs(120),
            1,
            10,
        );
        history.record(text("new"), "本机", 1, 10);
        let items = history.snapshot(1, 10);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].preview(), "new");
    }

    #[test]
    fn zero_limits_disable_and_clear_history() {
        let history = History::new();
        history.record(text("one"), "本机", 10, 10);
        assert_eq!(history.snapshot(10, 10).len(), 1);
        history.record(text("two"), "本机", 0, 10);
        assert!(history.snapshot(10, 10).is_empty());
        history.record(text("three"), "本机", 10, 0);
        assert!(history.snapshot(10, 10).is_empty());
    }

    #[test]
    fn file_items_drop_missing_paths_when_restoring() {
        let dir = std::env::temp_dir().join(format!(
            "lanpaster-history-test-{}",
            crate::config::random_hex(4)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, b"hello").unwrap();
        let history = History::new();
        history.record(
            HistoryPayload::Files {
                paths: vec![file.clone()],
                total_size: 5,
            },
            "本机",
            10,
            10,
        );
        assert!(history.snapshot(10, 10)[0].is_restorable());
        assert!(history.snapshot(10, 10)[0].to_remote().is_some());

        std::fs::remove_file(&file).unwrap();
        assert!(!history.snapshot(10, 10)[0].is_restorable());
        assert!(history.snapshot(10, 10)[0].to_remote().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
