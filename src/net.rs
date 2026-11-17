use crate::{
    clipboard::{ClipboardCommand, LocalClipboard, RemoteClipboard},
    config::{incoming_dir, Config},
    crypto::handshake,
    protocol::{sha256, ClipKind, ItemMeta, Message, CHUNK_SIZE, PROTOCOL_VERSION},
};
use anyhow::{bail, Context, Result};
use arc_swap::ArcSwap;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{mpsc, oneshot, Mutex},
    time::timeout,
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

#[derive(Debug)]
pub enum Control {
    Ready,
    Reject(String),
    Ack(bool, String),
}

#[derive(Clone)]
pub struct Peer {
    pub device_id: String,
    pub device_name: String,
    pub tx: mpsc::Sender<Message>,
    pub pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Control>>>>,
    pub conn_id: u64,
    pub cancel: CancellationToken,
    /// True when this connection is initiated by the lower device id.
    pub canonical: bool,
}

pub struct PeerManager {
    peers: Mutex<HashMap<String, Arc<Peer>>>,
}

impl Default for PeerManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerManager {
    pub fn new() -> Self {
        Self {
            peers: Mutex::new(HashMap::new()),
        }
    }

    pub async fn register(&self, peer: Arc<Peer>) -> bool {
        let mut peers = self.peers.lock().await;
        match peers.get(&peer.device_id) {
            None => {
                peers.insert(peer.device_id.clone(), peer);
                true
            }
            Some(old) => {
                if peer.canonical && !old.canonical {
                    info!("replacing duplicate connection to {}", old.device_name);
                    let old = peers.insert(peer.device_id.clone(), peer).unwrap();
                    old.cancel.cancel();
                    true
                } else {
                    debug!("keeping existing connection to {}", old.device_name);
                    peer.cancel.cancel();
                    false
                }
            }
        }
    }

    pub async fn unregister(&self, device_id: &str, conn_id: u64) {
        let mut peers = self.peers.lock().await;
        if peers.get(device_id).map(|p| p.conn_id) == Some(conn_id) {
            peers.remove(device_id);
        }
    }

    pub async fn snapshot(&self) -> Vec<Arc<Peer>> {
        self.peers.lock().await.values().cloned().collect()
    }

    pub async fn broadcast(&self, event: Arc<OutgoingEvent>) {
        let peers = self.snapshot().await;
        if peers.is_empty() {
            debug!("no peers connected; clipboard event dropped");
            return;
        }
        for peer in peers {
            let event = event.clone();
            tokio::spawn(async move {
                if let Err(err) = send_event_to_peer(peer.clone(), event).await {
                    debug!("send to {} failed: {err:#}", peer.device_name);
                }
            });
        }
    }
}

#[derive(Debug)]
pub struct SeenEvents {
    set: HashSet<u64>,
    order: VecDeque<u64>,
}

impl SeenEvents {
    pub fn new() -> Self {
        Self {
            set: HashSet::new(),
            order: VecDeque::new(),
        }
    }

    pub fn contains(&self, id: u64) -> bool {
        self.set.contains(&id)
    }

    pub fn insert(&mut self, id: u64) -> bool {
        if !self.set.insert(id) {
            return false;
        }
        self.order.push_back(id);
        while self.order.len() > 2048 {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        true
    }
}

impl Default for SeenEvents {
    fn default() -> Self {
        Self::new()
    }
}

pub struct AppContext {
    pub config: Arc<ArcSwap<Config>>,
    pub psk: Arc<Vec<u8>>,
    pub peers: Arc<PeerManager>,
    pub clipboard_cmd: std::sync::mpsc::Sender<ClipboardCommand>,
    pub seen: Arc<Mutex<SeenEvents>>,
    pub dialed: Arc<Mutex<HashSet<String>>>,
    pub sync_enabled: Arc<AtomicBool>,
    conn_seq: AtomicU64,
}

impl AppContext {
    pub fn new(
        config: Arc<ArcSwap<Config>>,
        clipboard_cmd: std::sync::mpsc::Sender<ClipboardCommand>,
    ) -> Result<Self> {
        let psk = Arc::new(config.load().psk_bytes()?);
        Ok(Self {
            config,
            psk,
            peers: Arc::new(PeerManager::new()),
            clipboard_cmd,
            seen: Arc::new(Mutex::new(SeenEvents::new())),
            dialed: Arc::new(Mutex::new(HashSet::new())),
            sync_enabled: Arc::new(AtomicBool::new(true)),
            conn_seq: AtomicU64::new(1),
        })
    }

    fn next_conn_id(&self) -> u64 {
        self.conn_seq.fetch_add(1, Ordering::Relaxed)
    }

    pub fn config(&self) -> Arc<Config> {
        self.config.load_full()
    }

    /// Apply a new config snapshot. Returns fields that still require a restart.
    pub fn apply_config(&self, new: Config) -> Vec<&'static str> {
        let old = self.config.load_full();
        let mut restart = Vec::new();
        if old.listen_port != new.listen_port {
            restart.push("listen_port");
        }
        if old.psk != new.psk {
            restart.push("psk");
        }
        if old.mdns_enabled != new.mdns_enabled {
            restart.push("mdns_enabled");
        }
        if old.peers != new.peers {
            restart.push("peers");
        }
        if old.device_id != new.device_id {
            restart.push("device_id");
        }
        self.config.store(Arc::new(new));
        restart
    }

    pub async fn ensure_dial(self: &Arc<Self>, addr: String) {
        {
            let mut dialed = self.dialed.lock().await;
            if !dialed.insert(addr.clone()) {
                return;
            }
        }
        let app = self.clone();
        tokio::spawn(async move {
            dial_loop(addr, app).await;
        });
    }
}

#[derive(Debug)]
pub enum OutgoingItem {
    Buffer {
        meta: ItemMeta,
        data: Arc<Vec<u8>>,
    },
    File {
        meta: ItemMeta,
        path: PathBuf,
    },
}

#[derive(Debug)]
pub struct OutgoingEvent {
    pub id: u64,
    pub origin: String,
    pub kind: ClipKind,
    pub items: Vec<OutgoingItem>,
    pub total_size: u64,
}

pub fn event_from_local(config: &Config, local: LocalClipboard) -> OutgoingEvent {
    let id = crate::config::random_u64();
    match local {
        LocalClipboard::Text { text, bytes, .. } => {
            let meta = ItemMeta {
                name: None,
                mime: "text/plain;charset=utf-8".to_string(),
                size: bytes.len() as u64,
                width: None,
                height: None,
                hash: Some(sha256(&bytes)),
            };
            let _ = text;
            OutgoingEvent {
                id,
                origin: config.device_id.clone(),
                kind: ClipKind::Text,
                total_size: bytes.len() as u64,
                items: vec![OutgoingItem::Buffer {
                    meta,
                    data: Arc::new(bytes),
                }],
            }
        }
        LocalClipboard::Image {
            mime,
            bytes,
            width,
            height,
            ..
        } => {
            let meta = ItemMeta {
                name: None,
                mime,
                size: bytes.len() as u64,
                width,
                height,
                hash: Some(sha256(&bytes)),
            };
            OutgoingEvent {
                id,
                origin: config.device_id.clone(),
                kind: ClipKind::Image,
                total_size: bytes.len() as u64,
                items: vec![OutgoingItem::Buffer {
                    meta,
                    data: Arc::new(bytes),
                }],
            }
        }
        LocalClipboard::Files {
            metas,
            total_size,
            ..
        } => {
            let items = metas
                .into_iter()
                .map(|m| OutgoingItem::File {
                    meta: ItemMeta {
                        name: Some(m.name),
                        mime: "application/octet-stream".to_string(),
                        size: m.size,
                        width: None,
                        height: None,
                        hash: None,
                    },
                    path: m.path,
                })
                .collect();
            OutgoingEvent {
                id,
                origin: config.device_id.clone(),
                kind: ClipKind::Files,
                items,
                total_size,
            }
        }
    }
}

async fn send_event_to_peer(peer: Arc<Peer>, event: Arc<OutgoingEvent>) -> Result<()> {
    let (ctl_tx, mut ctl_rx) = mpsc::channel(4);
    peer.pending.lock().await.insert(event.id, ctl_tx);

    let result = async {
        let offer = Message::Offer {
            id: event.id,
            origin: event.origin.clone(),
            kind: event.kind,
            items: event
                .items
                .iter()
                .map(|item| match item {
                    OutgoingItem::Buffer { meta, .. } | OutgoingItem::File { meta, .. } => {
                        meta.clone()
                    }
                })
                .collect(),
            total_size: event.total_size,
        };
        peer.tx
            .send(offer)
            .await
            .map_err(|_| anyhow::anyhow!("connection closed"))?;

        match timeout(Duration::from_secs(15), ctl_rx.recv()).await {
            Ok(Some(Control::Ready)) => {}
            Ok(Some(Control::Reject(reason))) => {
                debug!("peer {} rejected event: {reason}", peer.device_name);
                return Ok(());
            }
            Ok(Some(Control::Ack(_, _))) => bail!("unexpected ack before data"),
            Ok(None) => bail!("control channel closed"),
            Err(_) => bail!("timed out waiting for ready"),
        }

        for (index, item) in event.items.iter().enumerate() {
            let index = index as u32;
            let (meta, hash) = match item {
                OutgoingItem::Buffer { meta, data } => (
                    meta,
                    meta.hash
                        .clone()
                        .unwrap_or_else(|| sha256(data.as_slice())),
                ),
                OutgoingItem::File { meta, path } => (meta, hash_file(path).await?),
            };

            peer.tx
                .send(Message::ItemStart {
                    id: event.id,
                    index,
                    size: meta.size,
                    hash,
                })
                .await
                .map_err(|_| anyhow::anyhow!("connection closed"))?;

            match item {
                OutgoingItem::Buffer { data, .. } => {
                    let mut offset = 0u64;
                    for chunk in data.chunks(CHUNK_SIZE) {
                        peer.tx
                            .send(Message::ItemChunk {
                                id: event.id,
                                index,
                                offset,
                                data: chunk.to_vec(),
                            })
                            .await
                            .map_err(|_| anyhow::anyhow!("connection closed"))?;
                        offset += chunk.len() as u64;
                    }
                }
                OutgoingItem::File { path, .. } => {
                    let mut file = File::open(path)
                        .await
                        .with_context(|| format!("open {}", path.display()))?;
                    let mut offset = 0u64;
                    let mut buf = vec![0u8; CHUNK_SIZE];
                    loop {
                        let n = file.read(&mut buf).await?;
                        if n == 0 {
                            break;
                        }
                        peer.tx
                            .send(Message::ItemChunk {
                                id: event.id,
                                index,
                                offset,
                                data: buf[..n].to_vec(),
                            })
                            .await
                            .map_err(|_| anyhow::anyhow!("connection closed"))?;
                        offset += n as u64;
                    }
                    if offset != meta.size {
                        bail!("file changed while sending: {}", path.display());
                    }
                }
            }

            peer.tx
                .send(Message::ItemEnd {
                    id: event.id,
                    index,
                })
                .await
                .map_err(|_| anyhow::anyhow!("connection closed"))?;
        }

        peer.tx
            .send(Message::Done { id: event.id })
            .await
            .map_err(|_| anyhow::anyhow!("connection closed"))?;

        match timeout(Duration::from_secs(300), ctl_rx.recv()).await {
            Ok(Some(Control::Ack(true, _))) => Ok(()),
            Ok(Some(Control::Ack(false, reason))) => {
                warn!("peer {} failed to apply clipboard: {reason}", peer.device_name);
                Ok(())
            }
            Ok(Some(_)) => bail!("unexpected control message"),
            Ok(None) => bail!("control channel closed"),
            Err(_) => bail!("timed out waiting for ack"),
        }
    }
    .await;

    peer.pending.lock().await.remove(&event.id);
    result
}

async fn hash_file(path: &Path) -> Result<Vec<u8>> {
    let mut file = File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_vec())
}

pub async fn run_listener(app: Arc<AppContext>) -> Result<()> {
    let port = app.config.load().listen_port;
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    info!("listening on {addr}");
    loop {
        let (stream, peer) = listener.accept().await?;
        debug!("accepted connection from {peer}");
        let app = app.clone();
        tokio::spawn(async move {
            if let Err(err) = run_connection(stream, false, app).await {
                debug!("inbound connection ended: {err:#}");
            }
        });
    }
}

async fn dial_loop(addr: String, app: Arc<AppContext>) {
    let mut delay = 1u64;
    loop {
        match TcpStream::connect(&addr).await {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                info!("connected to {addr}");
                match run_connection(stream, true, app.clone()).await {
                    Ok(true) => delay = 1,
                    Ok(false) => {
                        debug!("duplicate connection to {addr}; backing off");
                        delay = 10;
                    }
                    Err(err) => {
                        debug!("connection to {addr} ended: {err:#}");
                        delay = 1;
                    }
                }
            }
            Err(err) => {
                debug!("connect to {addr} failed: {err}");
                delay = (delay * 2).min(30);
            }
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
    }
}

async fn run_connection(stream: TcpStream, initiator: bool, app: Arc<AppContext>) -> Result<bool> {
    stream.set_nodelay(true).ok();
    let (mut reader, mut writer) = handshake(stream, &app.psk, initiator).await?;

    let local = app.config.load_full();
    writer
        .send(&Message::Hello {
            device_id: local.device_id.clone(),
            device_name: local.device_name.clone(),
            version: PROTOCOL_VERSION,
        })
        .await?;

    let hello = reader.recv().await?;
    let (remote_id, remote_name, version) = match hello {
        Message::Hello {
            device_id,
            device_name,
            version,
        } => (device_id, device_name, version),
        other => bail!("expected hello, got {other:?}"),
    };
    if version != PROTOCOL_VERSION {
        bail!("protocol version mismatch: remote {version}, local {PROTOCOL_VERSION}");
    }
    if remote_id == local.device_id {
        bail!("refusing connection to self");
    }

    let (tx, mut rx) = mpsc::channel::<Message>(128);
    let pending = Arc::new(Mutex::new(HashMap::<u64, mpsc::Sender<Control>>::new()));
    let cancel = CancellationToken::new();
    let conn_id = app.next_conn_id();
    let canonical = initiator == (local.device_id.as_str() < remote_id.as_str());
    let peer = Arc::new(Peer {
        device_id: remote_id.clone(),
        device_name: remote_name.clone(),
        tx: tx.clone(),
        pending: pending.clone(),
        conn_id,
        cancel: cancel.clone(),
        canonical,
    });
    if !app.peers.register(peer).await {
        debug!("duplicate connection to {remote_name} rejected");
        cancel.cancel();
        return Ok(false);
    }
    info!("peer connected: {remote_name} ({remote_id})");

    let writer_cancel = cancel.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = writer_cancel.cancelled() => break,
                msg = rx.recv() => {
                    match msg {
                        Some(msg) => {
                            if let Err(err) = writer.send(&msg).await {
                                debug!("writer failed: {err:#}");
                                writer_cancel.cancel();
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    });

    let heartbeat_tx = tx.clone();
    let heartbeat_cancel = cancel.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            tokio::select! {
                _ = heartbeat_cancel.cancelled() => break,
                _ = interval.tick() => {
                    if heartbeat_tx.send(Message::Ping).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut incoming: HashMap<u64, IncomingEvent> = HashMap::new();
    let result = loop {
        tokio::select! {
            _ = cancel.cancelled() => break Ok(()),
            msg = reader.recv() => {
                let msg = match msg {
                    Ok(msg) => msg,
                    Err(err) => break Err(err),
                };
                if let Err(err) = handle_message(
                    msg,
                    &app,
                    &remote_id,
                    &tx,
                    &pending,
                    &mut incoming,
                ).await {
                    warn!("message handling failed: {err:#}");
                    break Err(err);
                }
            }
        }
    };

    cancel.cancel();
    app.peers.unregister(&remote_id, conn_id).await;
    for (_, event) in incoming {
        cleanup_event(event);
    }
    info!("peer disconnected: {remote_name}");
    result.map(|_| true)
}

async fn handle_message(
    msg: Message,
    app: &Arc<AppContext>,
    remote_id: &str,
    tx: &mpsc::Sender<Message>,
    pending: &Arc<Mutex<HashMap<u64, mpsc::Sender<Control>>>>,
    incoming: &mut HashMap<u64, IncomingEvent>,
) -> Result<()> {
    match msg {
        Message::Ping => {
            let _ = tx.send(Message::Pong).await;
        }
        Message::Pong => {}
        Message::Hello { .. } => bail!("unexpected hello"),
        Message::Ready { id } => dispatch(pending, id, Control::Ready).await,
        Message::Reject { id, reason } => dispatch(pending, id, Control::Reject(reason)).await,
        Message::Ack { id, ok, reason } => dispatch(pending, id, Control::Ack(ok, reason)).await,
        Message::Offer {
            id,
            origin,
            kind,
            items,
            total_size,
        } => {
            if origin != remote_id {
                let _ = tx
                    .send(Message::Reject {
                        id,
                        reason: "origin mismatch".to_string(),
                    })
                    .await;
                return Ok(());
            }
            if !app.sync_enabled.load(Ordering::Relaxed) {
                let _ = tx
                    .send(Message::Reject {
                        id,
                        reason: "sync disabled".to_string(),
                    })
                    .await;
                return Ok(());
            }
            if app.seen.lock().await.contains(id) || incoming.contains_key(&id) {
                let _ = tx
                    .send(Message::Reject {
                        id,
                        reason: "duplicate event".to_string(),
                    })
                    .await;
                return Ok(());
            }
            let cfg = app.config.load_full();
            match validate_offer(&cfg, kind, &items, total_size) {
                Ok(Some(temp_dir)) => {
                    let event = IncomingEvent::new(id, kind, items, total_size, Some(temp_dir));
                    incoming.insert(id, event);
                    let _ = tx.send(Message::Ready { id }).await;
                }
                Ok(None) => {
                    let event = IncomingEvent::new(id, kind, items, total_size, None);
                    incoming.insert(id, event);
                    let _ = tx.send(Message::Ready { id }).await;
                }
                Err(reason) => {
                    info!("reject clipboard event {id}: {reason}");
                    let _ = tx.send(Message::Reject { id, reason }).await;
                }
            }
        }
        Message::ItemStart {
            id,
            index,
            size,
            hash,
        } => {
            let event = incoming.get_mut(&id).context("unknown event")?;
            event.start_item(index as usize, size, hash).await?;
        }
        Message::ItemChunk {
            id,
            index,
            offset,
            data,
        } => {
            let event = incoming.get_mut(&id).context("unknown event")?;
            event.write_chunk(index as usize, offset, &data).await?;
        }
        Message::ItemEnd { id, index } => {
            let event = incoming.get_mut(&id).context("unknown event")?;
            event.finish_item(index as usize).await?;
        }
        Message::Done { id } => {
            let event = incoming.remove(&id).context("unknown event")?;
            let keep_files = event.kind == ClipKind::Files;
            let payload = match event.finalize() {
                Ok(payload) => payload,
                Err(err) => {
                    cleanup_event(event);
                    let _ = tx
                        .send(Message::Ack {
                            id,
                            ok: false,
                            reason: format!("{err:#}"),
                        })
                        .await;
                    return Ok(());
                }
            };
            let (respond, response) = oneshot::channel();
            if app
                .clipboard_cmd
                .send(ClipboardCommand::Write { payload, respond })
                .is_err()
            {
                cleanup_event(event);
                bail!("clipboard thread unavailable");
            }
            let applied = match timeout(Duration::from_secs(15), response).await {
                Ok(Ok(Ok(()))) => true,
                Ok(Ok(Err(err))) => {
                    warn!("write clipboard failed: {err:#}");
                    false
                }
                Ok(Err(_)) | Err(_) => false,
            };
            if applied {
                app.seen.lock().await.insert(id);
                if app.config.load().sound_enabled {
                    crate::sound::play();
                }
                if keep_files {
                    let cfg = app.config.load_full();
                    let max = cfg.cache_max_mb.saturating_mul(1024 * 1024);
                    crate::cache::cleanup_incoming(event.temp_dir.as_deref(), max);
                } else {
                    cleanup_event(event);
                }
                let _ = tx
                    .send(Message::Ack {
                        id,
                        ok: true,
                        reason: String::new(),
                    })
                    .await;
            } else {
                cleanup_event(event);
                let _ = tx
                    .send(Message::Ack {
                        id,
                        ok: false,
                        reason: "failed to write clipboard".to_string(),
                    })
                    .await;
            }
        }
    }
    Ok(())
}

async fn dispatch(
    pending: &Arc<Mutex<HashMap<u64, mpsc::Sender<Control>>>>,
    id: u64,
    control: Control,
) {
    let tx = pending.lock().await.get(&id).cloned();
    if let Some(tx) = tx {
        let _ = tx.send(control).await;
    }
}

fn validate_offer(
    config: &Config,
    kind: ClipKind,
    items: &[ItemMeta],
    total_size: u64,
) -> Result<Option<PathBuf>, String> {
    if items.is_empty() {
        return Err("empty clipboard event".to_string());
    }
    let sum: u64 = items.iter().map(|i| i.size).sum();
    if sum != total_size {
        return Err("total size mismatch".to_string());
    }
    match kind {
        ClipKind::Text => {
            if items.len() != 1 {
                return Err("text must have exactly one item".to_string());
            }
            let item = &items[0];
            if !item.mime.starts_with("text/") {
                return Err("invalid text mime".to_string());
            }
            if item.size > config.max_text_bytes() {
                return Err("text exceeds size limit".to_string());
            }
            Ok(None)
        }
        ClipKind::Image => {
            if items.len() != 1 {
                return Err("image must have exactly one item".to_string());
            }
            let item = &items[0];
            if !item.mime.starts_with("image/") {
                return Err("invalid image mime".to_string());
            }
            if item.size > config.max_image_bytes() {
                return Err("image exceeds size limit".to_string());
            }
            if let (Some(w), Some(h)) = (item.width, item.height) {
                if (w as u64).saturating_mul(h as u64) > config.max_image_pixels {
                    return Err("image exceeds pixel limit".to_string());
                }
            }
            Ok(None)
        }
        ClipKind::Files => {
            if !config.file_sync {
                return Err("file sync disabled".to_string());
            }
            if items.len() > config.max_files {
                return Err("too many files".to_string());
            }
            if total_size > config.max_batch_bytes() {
                return Err("batch exceeds size limit".to_string());
            }
            for item in items {
                if item.name.is_none() {
                    return Err("file item missing name".to_string());
                }
                if item.size > config.max_file_bytes() {
                    return Err(format!(
                        "file {} exceeds size limit",
                        item.name.as_deref().unwrap_or("?")
                    ));
                }
            }
            let dir = incoming_dir().join(format!(
                "{}-{}",
                std::process::id(),
                crate::config::random_hex(4)
            ));
            std::fs::create_dir_all(&dir).map_err(|e| format!("create temp dir: {e}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            }
            Ok(Some(dir))
        }
    }
}

struct IncomingEvent {
    kind: ClipKind,
    items: Vec<IncomingItem>,
    total_size: u64,
    received_total: u64,
    temp_dir: Option<PathBuf>,
}

struct IncomingItem {
    meta: ItemMeta,
    received: u64,
    hasher: Sha256,
    buffer: Option<Vec<u8>>,
    file: Option<File>,
    path: Option<PathBuf>,
    started: bool,
    finished: bool,
    expected_hash: Option<Vec<u8>>,
}

impl IncomingEvent {
    fn new(
        _id: u64,
        kind: ClipKind,
        items: Vec<ItemMeta>,
        total_size: u64,
        temp_dir: Option<PathBuf>,
    ) -> Self {
        let mut used = HashSet::new();
        let items = items
            .into_iter()
            .map(|meta| {
                let path = if kind == ClipKind::Files {
                    let name = sanitize_name(meta.name.as_deref().unwrap_or("file"), &mut used);
                    temp_dir.as_ref().map(|dir| dir.join(name))
                } else {
                    None
                };
                IncomingItem {
                    meta,
                    received: 0,
                    hasher: Sha256::new(),
                    buffer: None,
                    file: None,
                    path,
                    started: false,
                    finished: false,
                    expected_hash: None,
                }
            })
            .collect();
        Self {
            kind,
            items,
            total_size,
            received_total: 0,
            temp_dir,
        }
    }

    async fn start_item(&mut self, index: usize, size: u64, hash: Vec<u8>) -> Result<()> {
        let item = self.items.get_mut(index).context("invalid item index")?;
        if item.started {
            bail!("item already started");
        }
        if item.meta.size != size {
            bail!("item size mismatch");
        }
        item.expected_hash = Some(hash);
        item.started = true;
        match self.kind {
            ClipKind::Files => {
                let path = item.path.as_ref().context("missing file path")?.clone();
                let file = File::create(&path)
                    .await
                    .with_context(|| format!("create {}", path.display()))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = file
                        .set_permissions(std::fs::Permissions::from_mode(0o600))
                        .await;
                }
                item.file = Some(file);
            }
            ClipKind::Text | ClipKind::Image => {
                item.buffer = Some(Vec::with_capacity(size as usize));
            }
        }
        Ok(())
    }

    async fn write_chunk(&mut self, index: usize, offset: u64, data: &[u8]) -> Result<()> {
        let item = self.items.get_mut(index).context("invalid item index")?;
        if !item.started || item.finished {
            bail!("item not receiving");
        }
        if offset != item.received {
            bail!("unexpected chunk offset");
        }
        if item.received.saturating_add(data.len() as u64) > item.meta.size {
            bail!("chunk exceeds item size");
        }
        item.hasher.update(data);
        item.received += data.len() as u64;
        self.received_total += data.len() as u64;
        match self.kind {
            ClipKind::Files => {
                let file = item.file.as_mut().context("missing file")?;
                file.write_all(data).await?;
            }
            ClipKind::Text | ClipKind::Image => {
                item.buffer
                    .as_mut()
                    .context("missing buffer")?
                    .extend_from_slice(data);
            }
        }
        Ok(())
    }

    async fn finish_item(&mut self, index: usize) -> Result<()> {
        let item = self.items.get_mut(index).context("invalid item index")?;
        if !item.started || item.finished {
            bail!("item not receiving");
        }
        if item.received != item.meta.size {
            bail!("item size mismatch");
        }
        if let Some(file) = item.file.as_mut() {
            file.flush().await?;
            file.sync_all().await?;
        }
        let actual = item.hasher.clone().finalize().to_vec();
        if let Some(expected) = &item.expected_hash {
            if !expected.is_empty() && expected != &actual {
                bail!("hash mismatch");
            }
        }
        item.finished = true;
        Ok(())
    }

    fn finalize(&self) -> Result<RemoteClipboard> {
        if self.received_total != self.total_size {
            bail!("total size mismatch");
        }
        if self.items.iter().any(|i| !i.finished) {
            bail!("incomplete item");
        }
        match self.kind {
            ClipKind::Text => {
                let item = self.items.first().context("missing text item")?;
                let bytes = item.buffer.as_ref().context("missing text buffer")?;
                let text = String::from_utf8(bytes.clone()).context("text is not utf-8")?;
                Ok(RemoteClipboard::Text(text))
            }
            ClipKind::Image => {
                let item = self.items.first().context("missing image item")?;
                let bytes = item.buffer.as_ref().context("missing image buffer")?;
                Ok(RemoteClipboard::Image {
                    mime: item.meta.mime.clone(),
                    bytes: bytes.clone(),
                })
            }
            ClipKind::Files => {
                let paths = self
                    .items
                    .iter()
                    .map(|i| i.path.clone().context("missing file path"))
                    .collect::<Result<Vec<_>>>()?;
                Ok(RemoteClipboard::Files(paths))
            }
        }
    }
}

fn cleanup_event(event: IncomingEvent) {
    if let Some(dir) = event.temp_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
}

fn sanitize_name(name: &str, used: &mut HashSet<String>) -> String {
    let name = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("file")
        .trim()
        .trim_matches('.')
        .to_string();
    let mut name: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != '\0')
        .collect();
    if name.is_empty() {
        name = "file".to_string();
    }
    let upper = name.to_ascii_uppercase();
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if reserved.contains(&upper.as_str()) {
        name = format!("_{name}");
    }
    if name.chars().count() > 180 {
        name = name.chars().take(180).collect();
    }
    let mut candidate = name.clone();
    let mut n = 1;
    while used.contains(&candidate) {
        let (stem, ext) = match name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
            _ => (name.as_str(), String::new()),
        };
        candidate = format!("{stem}-{n}{ext}");
        n += 1;
    }
    used.insert(candidate.clone());
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_blocks_paths_and_reserved_names() {
        let mut used = HashSet::new();
        assert_eq!(sanitize_name("../../etc/passwd", &mut used), "passwd");
        assert_eq!(sanitize_name("C:\\temp\\CON", &mut used), "_CON");
        assert_eq!(sanitize_name("a.txt", &mut used), "a.txt");
        assert_eq!(sanitize_name("a.txt", &mut used), "a-1.txt");
        assert_eq!(sanitize_name("", &mut used), "file");
    }
}
