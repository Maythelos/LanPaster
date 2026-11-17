use anyhow::{bail, Context, Result};
use snow::{params::NoiseParams, Builder, TransportState};
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf},
    net::TcpStream,
    sync::Mutex,
};
use tracing::debug;

use crate::protocol::{decode_message, encode_message, Message, MAX_FRAME};

const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";

pub struct SecureReader {
    reader: ReadHalf<TcpStream>,
    noise: Arc<Mutex<TransportState>>,
}

pub struct SecureWriter {
    writer: WriteHalf<TcpStream>,
    noise: Arc<Mutex<TransportState>>,
}

pub fn generate_identity() -> (String, String) {
    let params: NoiseParams = NOISE_PARAMS.parse().expect("valid noise params");
    let builder = Builder::new(params);
    let keypair = builder.generate_keypair().expect("generate noise keypair");
    (hex::encode(keypair.private), hex::encode(keypair.public))
}

pub async fn handshake(
    stream: TcpStream,
    private_key: &[u8],
    initiator: bool,
) -> Result<(SecureReader, SecureWriter, Vec<u8>)> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let params: NoiseParams = NOISE_PARAMS.parse().context("parse noise params")?;
    let builder = Builder::new(params).local_private_key(private_key);
    let mut state = if initiator {
        builder.build_initiator().context("build noise initiator")?
    } else {
        builder.build_responder().context("build noise responder")?
    };

    let mut payload = vec![0u8; 65535];

    if initiator {
        let len = state.write_message(&[], &mut payload)?;
        write_raw(&mut writer, &payload[..len]).await?;

        let msg = read_raw(&mut reader).await?;
        state.read_message(&msg, &mut payload)?;

        let len = state.write_message(&[], &mut payload)?;
        write_raw(&mut writer, &payload[..len]).await?;
    } else {
        let msg = read_raw(&mut reader).await?;
        state.read_message(&msg, &mut payload)?;

        let len = state.write_message(&[], &mut payload)?;
        write_raw(&mut writer, &payload[..len]).await?;

        let msg = read_raw(&mut reader).await?;
        state.read_message(&msg, &mut payload)?;
    }

    if !state.is_handshake_finished() {
        bail!("noise handshake did not finish");
    }
    let transport = state.into_transport_mode().context("enter noise transport mode")?;
    let remote_static = transport
        .get_remote_static()
        .context("noise handshake did not expose remote static key")?
        .to_vec();
    let noise = Arc::new(Mutex::new(transport));
    Ok((
        SecureReader {
            reader,
            noise: noise.clone(),
        },
        SecureWriter { writer, noise },
        remote_static,
    ))
}

impl SecureWriter {
    pub async fn send(&mut self, msg: &Message) -> Result<()> {
        let plain = encode_message(msg)?;
        let mut ciphertext = vec![0u8; plain.len() + 16];
        let len = {
            let mut noise = self.noise.lock().await;
            noise
                .write_message(&plain, &mut ciphertext)
                .context("noise encrypt")?
        };
        if len > MAX_FRAME {
            bail!("encrypted frame too large: {len}");
        }
        self.writer.write_u32(len as u32).await?;
        self.writer.write_all(&ciphertext[..len]).await?;
        self.writer.flush().await?;
        Ok(())
    }
}

impl SecureReader {
    pub async fn recv(&mut self) -> Result<Message> {
        let ciphertext = read_raw(&mut self.reader).await?;
        let mut plain = vec![0u8; ciphertext.len()];
        let len = {
            let mut noise = self.noise.lock().await;
            noise
                .read_message(&ciphertext, &mut plain)
                .context("noise decrypt")?
        };
        debug!("received {} decrypted bytes", len);
        decode_message(&plain[..len])
    }
}

async fn write_raw<W: AsyncWriteExt + Unpin>(writer: &mut W, data: &[u8]) -> Result<()> {
    if data.is_empty() || data.len() > MAX_FRAME {
        bail!("invalid handshake frame length {}", data.len());
    }
    writer.write_u32(data.len() as u32).await?;
    writer.write_all(data).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_raw<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<Vec<u8>> {
    let len = reader.read_u32().await? as usize;
    if len == 0 || len > MAX_FRAME {
        bail!("invalid frame length {len}");
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}

