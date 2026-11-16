mod clipboard;
mod client;
mod config;
mod crypto;
mod mdns;
mod net;
mod protocol;
mod tray;
mod ui;

use crate::{
    config::{cache_dir, incoming_dir, Config},
    net::{event_from_local, run_listener, AppContext},
};
use anyhow::{Context, Result};
use std::{
    env,
    path::PathBuf,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::sync::mpsc as tokio_mpsc;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("run");
    match command {
        "run" => run(&args),
        "ui" => run_config_ui(),
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

fn run_config_ui() -> Result<()> {
    let (cfg, path) = load_config()?;
    cfg.validate()?;
    init_logging(&cfg.log_level);
    ui::run_config_window(cfg, path)
}

fn print_help() {
    println!(
        "LanPaster {}\n\nUSAGE:\n    lanpaster [run] [--headless]\n    lanpaster init\n    lanpaster ui\n    lanpaster config\n    lanpaster send <host:port> [text]\n    lanpaster send-file <host:port> <path>\n    lanpaster send-files <host:port> <path>...\n    lanpaster send-image <host:port> <path>\n    lanpaster version\n\nENV:\n    LANPASTER_CONFIG=/path/to/config.toml\n    RUST_LOG=info",
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

fn run(args: &[String]) -> Result<()> {
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

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    let app = runtime.block_on(start_services(cfg.clone()))?;

    let headless = args.iter().any(|a| a == "--headless")
        || env::var_os("LANPASTER_HEADLESS").is_some();
    if headless {
        info!("headless mode; press Ctrl-C to quit");
        runtime.block_on(async { tokio::signal::ctrl_c().await })?;
    } else {
        match tray::run_tray(app.clone(), runtime.handle().clone()) {
            Ok(()) => {}
            Err(err) => {
                error!("tray UI failed: {err:#}; falling back to headless mode");
                runtime.block_on(async { tokio::signal::ctrl_c().await })?;
            }
        }
    }

    info!("shutting down");
    runtime.shutdown_timeout(Duration::from_secs(2));
    Ok(())
}

async fn start_services(cfg: Config) -> Result<Arc<AppContext>> {
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

    let event_app = app.clone();
    let event_cfg = cfg.clone();
    tokio::spawn(async move {
        while let Some(local) = local_rx.recv().await {
            let event = event_from_local(&event_cfg, local);
            if !event_app.sync_enabled.load(Ordering::Relaxed) {
                info!("sync disabled; dropping local clipboard event");
                continue;
            }
            info!(
                "local clipboard event: {:?}, {} bytes, {} item(s)",
                event.kind,
                event.total_size,
                event.items.len()
            );
            event_app.peers.broadcast(Arc::new(event)).await;
        }
    });

    Ok(app)
}

fn init_logging(level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level.to_string()));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
