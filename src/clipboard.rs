use crate::config::Config;
use anyhow::Result;
use arc_swap::ArcSwap;
use clipboard_rs::{
    common::RustImage, Clipboard, ClipboardContext, ClipboardHandler, ClipboardWatcher,
    ClipboardWatcherContext, ContentFormat, RustImageData,
};
use sha2::{Digest, Sha256};
use std::{
    io::Cursor,
    path::PathBuf,
    sync::{mpsc, Arc},
    thread,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct LocalFileMeta {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub mtime: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum LocalClipboard {
    Text {
        text: String,
        bytes: Vec<u8>,
        hash: Vec<u8>,
    },
    Image {
        mime: String,
        bytes: Vec<u8>,
        width: Option<u32>,
        height: Option<u32>,
        hash: Vec<u8>,
    },
    Files {
        metas: Vec<LocalFileMeta>,
        total_size: u64,
        fingerprint: Vec<u8>,
    },
}

#[derive(Debug, Clone)]
pub enum RemoteClipboard {
    Text(String),
    Image {
        mime: String,
        bytes: Vec<u8>,
    },
    Files(Vec<PathBuf>),
}

pub enum ClipboardCommand {
    Write {
        payload: RemoteClipboard,
        respond: oneshot::Sender<Result<()>>,
    },
    /// Re-read the local clipboard and emit it as a local event.
    Push,
    /// The platform clipboard watcher reported a change.
    Changed,
}

struct ChangeHandler {
    tx: mpsc::Sender<ClipboardCommand>,
}

impl ClipboardHandler for ChangeHandler {
    fn on_clipboard_change(&mut self) {
        let _ = self.tx.send(ClipboardCommand::Changed);
    }
}

fn start_watcher(cmd_tx: mpsc::Sender<ClipboardCommand>) -> bool {
    match ClipboardWatcherContext::new_with_interval(Duration::from_millis(200)) {
        Ok(mut watcher) => {
            watcher.add_handler(ChangeHandler { tx: cmd_tx });
            match thread::Builder::new()
                .name("lanpaster-clipboard-watch".to_string())
                .spawn(move || watcher.start_watch())
            {
                Ok(_) => {
                    info!("clipboard watcher active");
                    true
                }
                Err(err) => {
                    warn!("clipboard watcher thread failed: {err}");
                    false
                }
            }
        }
        Err(err) => {
            warn!("clipboard watcher unavailable, falling back to polling: {err}");
            false
        }
    }
}

pub fn spawn(
    config: Arc<ArcSwap<Config>>,
    events: tokio_mpsc::UnboundedSender<LocalClipboard>,
) -> std::result::Result<std::sync::mpsc::Sender<ClipboardCommand>, String> {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let watcher_tx = cmd_tx.clone();
    thread::Builder::new()
        .name("lanpaster-clipboard".to_string())
        .spawn(move || {
            let watcher_active = start_watcher(watcher_tx);
            if let Err(err) = run(config, events, cmd_rx, watcher_active) {
                warn!("clipboard thread stopped: {err:#}");
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(cmd_tx)
}

fn run(
    config: Arc<ArcSwap<Config>>,
    events: tokio_mpsc::UnboundedSender<LocalClipboard>,
    commands: mpsc::Receiver<ClipboardCommand>,
    watcher_active: bool,
) -> Result<()> {
    let ctx = ClipboardContext::new().map_err(|e| anyhow::anyhow!("open clipboard: {e}"))?;
    let mut last_fp: Option<Vec<u8>> = None;
    let mut suppress_next = false;
    let mut last_image_mime: Option<String> = None;
    let tick = Duration::from_millis(200);
    let mut next_poll = Instant::now();

    loop {
        while let Ok(cmd) = commands.try_recv() {
            let cfg = config.load_full();
            if !apply_command(
                &ctx,
                &cfg,
                &events,
                &mut last_fp,
                &mut suppress_next,
                &mut last_image_mime,
                cmd,
            ) {
                return Ok(());
            }
        }

        let cfg = config.load_full();
        let fallback = if watcher_active {
            Duration::from_secs(10)
        } else {
            Duration::from_millis(cfg.poll_interval_ms.clamp(100, 5000))
        };
        if Instant::now() >= next_poll {
            emit_local(
                &ctx,
                &cfg,
                &events,
                &mut last_fp,
                &mut suppress_next,
                &mut last_image_mime,
            );
            next_poll = Instant::now() + fallback;
        }

        match commands.recv_timeout(tick) {
            Ok(ClipboardCommand::Changed) => {
                let cfg = config.load_full();
                emit_local(
                    &ctx,
                    &cfg,
                    &events,
                    &mut last_fp,
                    &mut suppress_next,
                    &mut last_image_mime,
                );
                next_poll = Instant::now() + fallback;
            }
            Ok(cmd) => {
                let cfg = config.load_full();
                if !apply_command(
                    &ctx,
                    &cfg,
                    &events,
                    &mut last_fp,
                    &mut suppress_next,
                    &mut last_image_mime,
                    cmd,
                ) {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

fn emit_local(
    ctx: &ClipboardContext,
    config: &Config,
    events: &tokio_mpsc::UnboundedSender<LocalClipboard>,
    last_fp: &mut Option<Vec<u8>>,
    suppress_next: &mut bool,
    last_image_mime: &mut Option<String>,
) {
    match read_local(ctx, config, last_image_mime.as_deref()) {
        Some(local) => {
            let fp = fingerprint_local(&local);
            if *suppress_next {
                *suppress_next = false;
                *last_fp = Some(fp);
                debug!("suppressed clipboard poll after remote write");
            } else if last_fp.as_ref() != Some(&fp) {
                *last_fp = Some(fp);
                let _ = events.send(local);
            }
        }
        None => {
            *last_fp = None;
            *suppress_next = false;
        }
    }
}

fn apply_command(
    ctx: &ClipboardContext,
    config: &Config,
    events: &tokio_mpsc::UnboundedSender<LocalClipboard>,
    last_fp: &mut Option<Vec<u8>>,
    suppress_next: &mut bool,
    last_image_mime: &mut Option<String>,
    command: ClipboardCommand,
) -> bool {
    match command {
        ClipboardCommand::Write { payload, respond } => {
            let image_mime = match &payload {
                RemoteClipboard::Image { mime, .. } => Some(mime.clone()),
                _ => None,
            };
            let result = write_remote(ctx, &payload);
            if result.is_ok() {
                if let Some(mime) = image_mime {
                    *last_image_mime = Some(mime);
                }
                *last_fp = None;
                *suppress_next = true;
            }
            let _ = respond.send(result);
            true
        }
        ClipboardCommand::Changed => true,
        ClipboardCommand::Push => {
            if let Some(local) = read_local(ctx, config, last_image_mime.as_deref()) {
                if let LocalClipboard::Image { mime, .. } = &local {
                    *last_image_mime = Some(mime.clone());
                }
                *last_fp = Some(fingerprint_local(&local));
                *suppress_next = false;
                if events.send(local).is_err() {
                    return false;
                }
            }
            true
        }
    }
}

fn read_local(ctx: &ClipboardContext, config: &Config, last_image_mime: Option<&str>) -> Option<LocalClipboard> {
    if ctx.has(ContentFormat::Files) {
        if !config.file_sync {
            debug!("file sync disabled; ignoring file clipboard");
            return None;
        }
        if let Some(files) = read_files(ctx, config) {
            return Some(files);
        }
    }
    if let Some(image) = read_image(ctx, config, last_image_mime) {
        return Some(image);
    }
    read_text(ctx, config)
}

fn read_files(ctx: &ClipboardContext, config: &Config) -> Option<LocalClipboard> {
    if !ctx.has(ContentFormat::Files) {
        return None;
    }
    let files = ctx.get_files().ok()?;
    if files.is_empty() {
        return None;
    }
    if files.len() > config.max_files {
        debug!("skip clipboard files: {} files exceeds limit {}", files.len(), config.max_files);
        return None;
    }

    let mut metas = Vec::with_capacity(files.len());
    let mut total = 0u64;
    for raw in files {
        let path = PathBuf::from(raw);
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(err) => {
                debug!("skip file {}: {err}", path.display());
                return None;
            }
        };
        if meta.file_type().is_symlink() || !meta.is_file() {
            debug!("skip clipboard files: only regular files are supported ({})", path.display());
            return None;
        }
        if meta.len() > config.max_file_bytes() {
            debug!(
                "skip clipboard files: {} is {} bytes, over per-file limit",
                path.display(),
                meta.len()
            );
            return None;
        }
        total = total.saturating_add(meta.len());
        if total > config.max_batch_bytes() {
            debug!("skip clipboard files: total size exceeds batch limit");
            return None;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        metas.push(LocalFileMeta {
            name,
            path,
            size: meta.len(),
            mtime,
        });
    }

    let fingerprint = fingerprint_files(&metas);
    Some(LocalClipboard::Files {
        metas,
        total_size: total,
        fingerprint,
    })
}

fn read_image(
    ctx: &ClipboardContext,
    config: &Config,
    preferred_mime: Option<&str>,
) -> Option<LocalClipboard> {
    if !ctx.has(ContentFormat::Image) {
        return None;
    }

    if let Ok(formats) = ctx.available_formats() {
        let preferred = preferred_mime.and_then(|mime| {
            let source = platform_image_format(mime);
            formats
                .iter()
                .find(|f| f.eq_ignore_ascii_case(source))
                .map(|f| (f.clone(), mime.to_string()))
        });
        let chosen = preferred.or_else(|| find_image_format(&formats));
        if let Some((source_format, mime)) = chosen {
            if let Ok(bytes) = ctx.get_buffer(&source_format) {
                if bytes.is_empty() {
                    return None;
                }
                let limit = config.max_file_bytes().min(config.max_batch_bytes());
                if bytes.len() as u64 > limit {
                    debug!(
                        "skip image: {} bytes exceeds file size limit",
                        bytes.len()
                    );
                    return None;
                }
                let hash = sha256_prefixed(b"image", &bytes);
                return Some(LocalClipboard::Image {
                    mime,
                    bytes,
                    width: None,
                    height: None,
                    hash,
                });
            }
        }
    }

    let data = ctx.get_image().ok()?;
    let dynamic = data.get_dynamic_image().ok()?;
    let (width, height) = (dynamic.width(), dynamic.height());
    let mut png = Vec::new();
    dynamic
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    let limit = config.max_file_bytes().min(config.max_batch_bytes());
    if png.len() as u64 > limit {
        debug!("skip image: encoded PNG exceeds file size limit");
        return None;
    }
    let hash = sha256_prefixed(b"image", &png);
    Some(LocalClipboard::Image {
        mime: "image/png".to_string(),
        bytes: png,
        width: Some(width),
        height: Some(height),
        hash,
    })
}

fn read_text(ctx: &ClipboardContext, config: &Config) -> Option<LocalClipboard> {
    let text = ctx.get_text().ok()?;
    if text.is_empty() {
        return None;
    }
    let bytes = text.as_bytes().to_vec();
    if bytes.len() as u64 > config.max_text_bytes() {
        debug!("skip text: {} bytes exceeds text limit", bytes.len());
        return None;
    }
    let hash = sha256_prefixed(b"text", &bytes);
    Some(LocalClipboard::Text { text, bytes, hash })
}

fn write_remote(ctx: &ClipboardContext, payload: &RemoteClipboard) -> Result<()> {
    match payload {
        RemoteClipboard::Text(text) => retry_clipboard(
            || ctx.set_text(text.clone()),
            "set clipboard text",
        ),
        RemoteClipboard::Image { mime, bytes } => {
            let format = platform_image_format(mime);
            let mut last_err = String::new();
            for _ in 0..3 {
                match ctx.set_buffer(format, bytes.clone()) {
                    Ok(()) => return Ok(()),
                    Err(err) => last_err = err.to_string(),
                }
                match RustImageData::from_bytes(bytes) {
                    Ok(image) => match ctx.set_image(image) {
                        Ok(()) => return Ok(()),
                        Err(err) => last_err = err.to_string(),
                    },
                    Err(err) => {
                        if last_err.is_empty() {
                            last_err = err.to_string();
                        }
                    }
                }
                thread::sleep(Duration::from_millis(60));
            }
            Err(anyhow::anyhow!("set clipboard image: {last_err}"))
        }
        RemoteClipboard::Files(paths) => {
            let files: Vec<String> = paths
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            retry_clipboard(|| ctx.set_files(files.clone()), "set clipboard files")
        }
    }
}

fn retry_clipboard<F>(mut f: F, what: &str) -> Result<()>
where
    F: FnMut() -> clipboard_rs::common::Result<()>,
{
    let mut last_err = String::new();
    for _ in 0..3 {
        match f() {
            Ok(()) => return Ok(()),
            Err(err) => last_err = err.to_string(),
        }
        thread::sleep(Duration::from_millis(60));
    }
    Err(anyhow::anyhow!("{what}: {last_err}"))
}

fn fingerprint_local(local: &LocalClipboard) -> Vec<u8> {
    match local {
        LocalClipboard::Text { hash, .. } | LocalClipboard::Image { hash, .. } => hash.clone(),
        LocalClipboard::Files { fingerprint, .. } => fingerprint.clone(),
    }
}

fn fingerprint_files(metas: &[LocalFileMeta]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(b"files");
    for meta in metas {
        hasher.update(meta.name.as_bytes());
        hasher.update(meta.size.to_le_bytes());
        hasher.update(meta.mtime.unwrap_or_default().to_le_bytes());
    }
    hasher.finalize().to_vec()
}

fn sha256_prefixed(prefix: &[u8], data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(prefix);
    hasher.update(data);
    hasher.finalize().to_vec()
}


fn find_image_format(formats: &[String]) -> Option<(String, String)> {
    const CANDIDATES: &[(&str, &str)] = &[
        ("public.png", "image/png"),
        ("PNG", "image/png"),
        ("image/png", "image/png"),
        ("public.jpeg", "image/jpeg"),
        ("public.jpg", "image/jpeg"),
        ("JFIF", "image/jpeg"),
        ("JPEG", "image/jpeg"),
        ("image/jpeg", "image/jpeg"),
        ("org.webmproject.webp", "image/webp"),
        ("public.webp", "image/webp"),
        ("WEBP", "image/webp"),
        ("image/webp", "image/webp"),
        ("com.compuserve.gif", "image/gif"),
        ("GIF", "image/gif"),
        ("image/gif", "image/gif"),
        ("public.heic", "image/heic"),
        ("HEIC", "image/heic"),
        ("image/heic", "image/heic"),
        ("public.tiff", "image/tiff"),
        ("TIFF", "image/tiff"),
        ("image/tiff", "image/tiff"),
    ];
    for (source, mime) in CANDIDATES {
        if formats.iter().any(|f| f.eq_ignore_ascii_case(source)) {
            return Some((source.to_string(), mime.to_string()));
        }
    }
    None
}

fn platform_image_format(mime: &str) -> &str {
    #[cfg(target_os = "macos")]
    {
        match mime {
            "image/png" => "public.png",
            "image/jpeg" => "public.jpeg",
            "image/tiff" => "public.tiff",
            "image/heic" => "public.heic",
            "image/gif" => "com.compuserve.gif",
            "image/webp" => "org.webmproject.webp",
            _ => mime,
        }
    }
    #[cfg(target_os = "windows")]
    {
        match mime {
            "image/png" => "PNG",
            "image/jpeg" => "JFIF",
            "image/tiff" => "TIFF",
            "image/heic" => "HEIC",
            "image/gif" => "GIF",
            "image/webp" => "WEBP",
            _ => mime,
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        mime
    }
}
