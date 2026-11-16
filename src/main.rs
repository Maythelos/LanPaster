mod clipboard;
mod client;
mod config;
mod crypto;
mod mdns;
mod net;
mod protocol;

use crate::{
    config::{cache_dir, incoming_dir, Config},
    net::{event_from_local, run_listener, AppContext},
};
use anyhow::{Context, Result};
use std::{env, path::PathBuf, sync::Arc};
use tokio::sync::mpsc as tokio_mpsc;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("run");
    match command {
        "run" => run(),
        "send" => send(&args),
        "send-file" => send_file(&args),
        "send-files" => send_files(&args),
        "send-image" => send_image(&args),
        "init" => init(),
        "config" | "print-config" => print_config(),
        "version" | "--version" | "-V" => {
            println!("lanpaster {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => anyhow::bail!("unknown command {other:?}; try `lanpaster help`"),
    }
}

fn send(args: &[String]) -> Result<()> {
    let addr = args
        .get(1)
        .ok_or_else(|| anyhow::anyhow!("usage: lanpaster send <host:port> [text]"))?
        .clone();
    let text = if args.len() > 2 {
        args[2..].join(" ")
    } else {
        client::read_stdin()?
    };
    let (cfg, _) = load_config()?;
    cfg.validate()?;
    init_logging(&cfg.log_level);
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(client::send_text(&cfg, &addr, text))
}

fn send_file(args: &[String]) -> Result<()> {
    let addr = args.get(1).ok_or_else(|| anyhow::anyhow!("usage: lanpaster send-file <host:port> <path>"))?;
    let path = args.get(2).ok_or_else(|| anyhow::anyhow!("usage: lanpaster send-file <host:port> <path>"))?;
    let (cfg, _) = load_config()?;
    cfg.validate()?;
    init_logging(&cfg.log_level);
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(client::send_file(&cfg, addr, std::path::Path::new(path)))
}

fn send_files(args: &[String]) -> Result<()> {
    let addr = args.get(1).ok_or_else(|| anyhow::anyhow!("usage: lanpaster send-files <host:port> <path>..."))?;
    if args.len() < 3 {
        anyhow::bail!("usage: lanpaster send-files <host:port> <path>...");
    }
    let (cfg, _) = load_config()?;
    cfg.validate()?;
    init_logging(&cfg.log_level);
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(client::send_files(&cfg, addr, &args[2..]))
}

fn send_image(args: &[String]) -> Result<()> {
    let addr = args.get(1).ok_or_else(|| anyhow::anyhow!("usage: lanpaster send-image <host:port> <path>"))?;
    let path = args.get(2).ok_or_else(|| anyhow::anyhow!("usage: lanpaster send-image <host:port> <path>"))?;
    let (cfg, _) = load_config()?;
    cfg.validate()?;
    init_logging(&cfg.log_level);
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(client::send_image(&cfg, addr, std::path::Path::new(path)))
}

fn print_help() {
    println!(
        "LanPaster {}\n\nUSAGE:\n    lanpaster [run]\n    lanpaster init\n    lanpaster config\n    lanpaster send <host:port> [text]\n    lanpaster send-file <host:port> <path>\n    lanpaster send-files <host:port> <path>...\n    lanpaster send-image <host:port> <path>\n    lanpaster version\n\nENV:\n    LANPASTER_CONFIG=/path/to/config.toml\n    RUST_LOG=info",
        env!("CARGO_PKG_VERSION")
    );
}

fn config_override() -> Option<PathBuf> {
    env::var_os("LANPASTER_CONFIG").map(PathBuf::from)
}

fn load_config() -> Result<(Config, PathBuf)> {
    let path = config_override().unwrap_or_else(config::config_path);
    Config::load_or_create_at(&path)
}

fn init() -> Result<()> {
    let (cfg, path) = load_config()?;
    cfg.validate()?;
    println!("config: {}", path.display());
    println!("device: {} ({})", cfg.device_name, cfg.device_id);
    println!("psk:    {}", cfg.psk);
    println!("\nCopy the psk to other devices, or copy this config file.");
    Ok(())
}

fn print_config() -> Result<()> {
    let (cfg, path) = load_config()?;
    println!("# {}", path.display());
    print!("{}", toml::to_string_pretty(&cfg)?);
    Ok(())
}

#[tokio::main]
async fn run() -> Result<()> {
    let (cfg, path) = load_config()?;
    cfg.validate()?;
    init_logging(&cfg.log_level);

    info!("config: {}", path.display());
    info!("device: {} ({})", cfg.device_name, cfg.device_id);
    info!("psk:    {}", cfg.psk);
    info!("listening port: {}", cfg.listen_port);
    if cfg.peers.is_empty() {
        info!("no manual peers configured; add peers = [\"192.168.1.10:39393\"] to config");
    }

    let cache = cache_dir();
    std::fs::create_dir_all(&cache)?;
    let incoming = incoming_dir();
    let _ = std::fs::remove_dir_all(&incoming);
    std::fs::create_dir_all(&incoming)?;

    let (local_tx, mut local_rx) = tokio_mpsc::unbounded_channel();
    let clipboard_cmd = clipboard::spawn(cfg.clone(), local_tx)
        .map_err(|e| anyhow::anyhow!("clipboard thread: {e}"))?;
    let app = Arc::new(AppContext::new(cfg.clone(), clipboard_cmd)?);

    for peer in &cfg.peers {
        app.ensure_dial(peer.clone()).await;
    }

    if cfg.mdns_enabled {
        mdns::spawn(cfg.clone(), app.clone(), tokio::runtime::Handle::current());
    }

    let listener_app = app.clone();
    tokio::spawn(async move {
        if let Err(err) = run_listener(listener_app).await {
            error!("listener stopped: {err:#}");
        }
    });

    tokio::spawn(async move {
        while let Some(local) = local_rx.recv().await {
            let event = event_from_local(&cfg, local);
            info!(
                "local clipboard event: {:?}, {} bytes, {} item(s)",
                event.kind,
                event.total_size,
                event.items.len()
            );
            app.peers.broadcast(Arc::new(event)).await;
        }
    });

    info!("LanPaster is running. Press Ctrl-C to quit.");
    tokio::signal::ctrl_c()
        .await
        .context("wait for ctrl-c")?;
    info!("shutting down");
    Ok(())
}

fn init_logging(level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level.to_string()));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
