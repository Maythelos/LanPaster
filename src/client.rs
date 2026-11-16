use crate::{
    config::Config,
    crypto::handshake,
    protocol::{sha256, ClipKind, ItemMeta, Message, CHUNK_SIZE, PROTOCOL_VERSION},
};
use anyhow::{bail, Context, Result};
use std::{io::Read, path::Path};
use tokio::net::TcpStream;

pub async fn send_text(config: &Config, addr: &str, text: String) -> Result<()> {
    let bytes = text.into_bytes();
    let hash = sha256(&bytes);
    let meta = ItemMeta {
        name: None,
        mime: "text/plain;charset=utf-8".to_string(),
        size: bytes.len() as u64,
        width: None,
        height: None,
        hash: Some(hash),
    };
    send_event(config, addr, ClipKind::Text, vec![(meta, bytes)]).await
}

pub async fn send_file(config: &Config, addr: &str, path: &Path) -> Result<()> {
    let data = tokio::fs::read(path)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let meta = ItemMeta {
        name: Some(name),
        mime: "application/octet-stream".to_string(),
        size: data.len() as u64,
        width: None,
        height: None,
        hash: Some(sha256(&data)),
    };
    send_event(config, addr, ClipKind::Files, vec![(meta, data)]).await
}

pub async fn send_files(config: &Config, addr: &str, paths: &[String]) -> Result<()> {
    let mut items = Vec::with_capacity(paths.len());
    for path in paths {
        let path = Path::new(path);
        let data = tokio::fs::read(path)
            .await
            .with_context(|| format!("read {}", path.display()))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        let meta = ItemMeta {
            name: Some(name),
            mime: "application/octet-stream".to_string(),
            size: data.len() as u64,
            width: None,
            height: None,
            hash: Some(sha256(&data)),
        };
        items.push((meta, data));
    }
    send_event(config, addr, ClipKind::Files, items).await
}

pub async fn send_image(config: &Config, addr: &str, path: &Path) -> Result<()> {
    let data = tokio::fs::read(path)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    let (width, height) = image::load_from_memory(&data)
        .map(|img| (Some(img.width()), Some(img.height())))
        .unwrap_or((None, None));
    let mime = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("tif") | Some("tiff") => "image/tiff",
        Some("heic") => "image/heic",
        _ => "image/png",
    }
    .to_string();
    let meta = ItemMeta {
        name: None,
        mime,
        size: data.len() as u64,
        width,
        height,
        hash: Some(sha256(&data)),
    };
    send_event(config, addr, ClipKind::Image, vec![(meta, data)]).await
}

async fn send_event(
    config: &Config,
    addr: &str,
    kind: ClipKind,
    items: Vec<(ItemMeta, Vec<u8>)>,
) -> Result<()> {
    let psk = config.psk_bytes()?;
    let stream = TcpStream::connect(addr)
        .await
        .with_context(|| format!("connect {addr}"))?;
    let (mut reader, mut writer) = handshake(stream, &psk, true).await?;

    writer
        .send(&Message::Hello {
            device_id: config.device_id.clone(),
            device_name: config.device_name.clone(),
            version: PROTOCOL_VERSION,
        })
        .await?;
    match reader.recv().await? {
        Message::Hello { device_name, .. } => println!("connected to {device_name}"),
        other => bail!("expected hello, got {other:?}"),
    }

    let id = crate::config::random_u64();
    let total_size: u64 = items.iter().map(|(meta, _)| meta.size).sum();
    writer
        .send(&Message::Offer {
            id,
            origin: config.device_id.clone(),
            kind,
            items: items.iter().map(|(meta, _)| meta.clone()).collect(),
            total_size,
        })
        .await?;

    loop {
        match reader.recv().await? {
            Message::Ready { .. } => break,
            Message::Reject { reason, .. } => bail!("peer rejected: {reason}"),
            Message::Ping => writer.send(&Message::Pong).await?,
            Message::Pong => {}
            other => bail!("unexpected response: {other:?}"),
        }
    }

    for (index, (meta, data)) in items.iter().enumerate() {
        let index = index as u32;
        writer
            .send(&Message::ItemStart {
                id,
                index,
                size: meta.size,
                hash: meta.hash.clone().unwrap_or_else(|| sha256(data)),
            })
            .await?;
        let mut offset = 0u64;
        for chunk in data.chunks(CHUNK_SIZE) {
            writer
                .send(&Message::ItemChunk {
                    id,
                    index,
                    offset,
                    data: chunk.to_vec(),
                })
                .await?;
            offset += chunk.len() as u64;
        }
        writer.send(&Message::ItemEnd { id, index }).await?;
    }
    writer.send(&Message::Done { id }).await?;

    loop {
        match reader.recv().await? {
            Message::Ack { ok: true, .. } => {
                println!("sent {total_size} bytes");
                return Ok(());
            }
            Message::Ack { ok: false, reason, .. } => bail!("peer failed: {reason}"),
            Message::Ping => writer.send(&Message::Pong).await?,
            Message::Pong => {}
            other => bail!("unexpected ack: {other:?}"),
        }
    }
}

pub fn read_stdin() -> Result<String> {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text)?;
    Ok(text)
}
