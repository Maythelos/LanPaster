use anyhow::{bail, Result};
use bincode::{Decode, Encode};
use sha2::{Digest, Sha256};

pub const PROTOCOL_VERSION: u32 = 2;
/// Maximum encrypted/plain frame payload. Noise overhead is 16 bytes.
pub const MAX_FRAME: usize = 60_000;
/// Clipboard/file chunk size. Keep well below MAX_FRAME.
pub const CHUNK_SIZE: usize = 48 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub enum ClipKind {
    Text,
    Image,
    Files,
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct ItemMeta {
    pub name: Option<String>,
    pub mime: String,
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Optional for files (computed while streaming).
    pub hash: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Encode, Decode)]
pub enum Message {
    Hello {
        device_id: String,
        device_name: String,
        version: u32,
        /// 发送方当前是否仍然信任接收方。双方都会在 Hello 里声明，
        /// 任一方解除配对后，另一方可以立即跟进删除。
        trusts_me: bool,
    },
    Offer {
        id: u64,
        origin: String,
        kind: ClipKind,
        items: Vec<ItemMeta>,
        total_size: u64,
    },
    Ready {
        id: u64,
    },
    Reject {
        id: u64,
        reason: String,
    },
    ItemStart {
        id: u64,
        index: u32,
        size: u64,
        hash: Vec<u8>,
    },
    ItemChunk {
        id: u64,
        index: u32,
        offset: u64,
        data: Vec<u8>,
    },
    ItemEnd {
        id: u64,
        index: u32,
    },
    Done {
        id: u64,
    },
    Ack {
        id: u64,
        ok: bool,
        reason: String,
    },
    PairRequest,
    PairPending,
    PairAccepted,
    PairDenied {
        reason: String,
    },
    /// 通知对端：本机已解除配对。接收方应删除对应信任记录并断开。
    Unpair {
        reason: String,
    },
    Ping,
    Pong,
}

pub fn encode_message(msg: &Message) -> Result<Vec<u8>> {
    let bytes = bincode::encode_to_vec(msg, bincode::config::standard())?;
    if bytes.len() > MAX_FRAME {
        bail!("encoded message too large: {} bytes", bytes.len());
    }
    Ok(bytes)
}

pub fn decode_message(buf: &[u8]) -> Result<Message> {
    let (msg, _): (Message, usize) = bincode::decode_from_slice(
        buf,
        bincode::config::standard().with_limit::<MAX_FRAME>(),
    )?;
    Ok(msg)
}

pub fn sha256(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_roundtrip() {
        let msg = Message::Offer {
            id: 42,
            origin: "dev".to_string(),
            kind: ClipKind::Files,
            items: vec![ItemMeta {
                name: Some("a.txt".to_string()),
                mime: "application/octet-stream".to_string(),
                size: 3,
                width: None,
                height: None,
                hash: Some(vec![1, 2, 3]),
            }],
            total_size: 3,
        };
        let bytes = encode_message(&msg).unwrap();
        let decoded = decode_message(&bytes).unwrap();
        match decoded {
            Message::Offer { id, total_size, .. } => {
                assert_eq!(id, 42);
                assert_eq!(total_size, 3);
            }
            _ => panic!("wrong message"),
        }
    }
}
