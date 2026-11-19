mod autostart;
mod cache;
mod clipboard;
mod config;
mod crypto;
mod history;
mod instance;
mod mdns;
mod net;
mod pairing;
mod protocol;
mod sound;
mod tray;
mod ui;

use crate::{
    config::{cache_dir, incoming_dir, Config},
    net::{event_from_local, run_listener, AppContext},
};
use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use std::{
    env,
    path::{Path, PathBuf},
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::sync::mpsc as tokio_mpsc;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("run");
    match command {
        "run" => run(&args),
        "ui" => run_config_ui(&args),
        "beep" => { sound::test_beep(); Ok(()) }
        "autostart" => autostart_cmd(&args),
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

fn autostart_cmd(args: &[String]) -> Result<()> {
    match args.get(1).map(String::as_str) {
        Some("enable") => {
            autostart::set(true)?;
            println!("autostart enabled");
        }
        Some("disable") => {
            autostart::set(false)?;
            println!("autostart disabled");
        }
        Some("status") | None => {
            println!(
                "autostart: {}",
                if autostart::is_enabled() { "enabled" } else { "disabled" }
            );
        }
        Some(other) => anyhow::bail!("unknown autostart action {other:?}"),
    }
    Ok(())
}

fn run_config_ui(args: &[String]) -> Result<()> {
    let panel = match args.get(1) {
        Some(value) => ui::Panel::parse(value)
            .ok_or_else(|| anyhow::anyhow!("unknown panel {value:?}; use connection or local"))?,
        None => ui::Panel::Connection,
    };
    let (cfg, path) = load_config()?;
    cfg.validate()?;
    init_logging(&cfg.log_level);
    ui::run_config_window(cfg, path, panel)
}

fn print_help() {
    println!(
        "LanPaster {}\n\nUSAGE:\n    lanpaster [run] [--headless]\n    lanpaster init\n    lanpaster ui [connection|local]\n    lanpaster beep\n    lanpaster autostart <enable|disable|status>\n    lanpaster config\n    lanpaster version\n\nENV:\n    LANPASTER_CONFIG=/path/to/config.toml\n    RUST_LOG=info",
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
    println!(
        "fingerprint: {}",
        pairing::PairingManager::fingerprint(&cfg.public_key_bytes()?)
    );
    println!("\nOther devices will ask for approval on first connection.");
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

    let Some(_instance_lock) = instance::acquire(&path)? else {
        println!(
            "LanPaster is already running for config {}",
            path.display()
        );
        return Ok(());
    };

    info!("config: {}", path.display());
    info!("device: {} ({})", cfg.device_name, cfg.device_id);
    info!(
        "fingerprint: {}",
        pairing::PairingManager::fingerprint(&cfg.public_key_bytes()?)
    );
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
    let app = runtime.block_on(start_services(cfg.clone(), path.clone()))?;

    let headless = args.iter().any(|a| a == "--headless")
        || env::var_os("LANPASTER_HEADLESS").is_some();
    if headless {
        info!("headless mode; press Ctrl-C to quit");
        runtime.block_on(async { tokio::signal::ctrl_c().await })?;
    } else {
        match tray::run_tray(app.clone(), runtime.handle().clone(), path.clone()) {
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

async fn start_services(cfg: Config, config_path: PathBuf) -> Result<Arc<AppContext>> {
    let (local_tx, mut local_rx) = tokio_mpsc::unbounded_channel();
    let config = Arc::new(ArcSwap::from_pointee(cfg.clone()));
    let history = Arc::new(history::History::new());
    let clipboard_cmd = clipboard::spawn(config.clone(), local_tx, history.clone())
        .map_err(|e| anyhow::anyhow!("clipboard thread: {e}"))?;
    let trusted_path = config_path.with_extension("trusted.toml");
    let pairing = Arc::new(pairing::PairingManager::new(trusted_path)?);
    let app = Arc::new(AppContext::new(config, clipboard_cmd, pairing, history)?);

    // Enforce the history age window even when no tray UI is polling it
    // (headless mode). The tray also prunes on every refresh.
    {
        let history_app = app.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                let cfg = history_app.config();
                history_app
                    .history
                    .prune(cfg.history_minutes, cfg.history_limit);
            }
        });
    }

    // The settings window is a separate process, so watch the trusted file for
    // removals. Drop live connections immediately and make sure the removed
    // device is remembered, instead of waiting for the next reconnect.
    {
        let watch_app = app.clone();
        tokio::spawn(async move {
            let mut last = watch_app.pairing.trusted_list();
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let current = watch_app.pairing.trusted_list();
                let removed: Vec<_> = last
                    .iter()
                    .filter(|old| {
                        !current.iter().any(|new| {
                            new.device_id == old.device_id
                                && new.public_key.eq_ignore_ascii_case(&old.public_key)
                        })
                    })
                    .cloned()
                    .collect();
                if removed.is_empty() {
                    last = current;
                    continue;
                }
                for peer in &removed {
                    info!(
                        "trusted peer removed: {} ({})",
                        peer.name, peer.device_id
                    );
                    let _ = watch_app.pairing.revoke(
                        &peer.device_id,
                        &peer.name,
                        &peer.public_key,
                    );
                    watch_app
                        .unpair_peer(&peer.device_id, "本机已移除该设备")
                        .await;
                }
                last = watch_app.pairing.trusted_list();
            }
        });
    }

    for peer in &cfg.peers {
        app.ensure_dial(peer.clone()).await;
    }

    if cfg.mdns_enabled {
        mdns::spawn(cfg.clone(), app.clone());
    }

    // Watch the config file written by the on-demand config window and
    // hot-apply the settings that do not require restarting the network layer.
    {
        let watch_app = app.clone();
        let watch_path = config_path.clone();
        tokio::spawn(async move {
            let mut last = config_stamp(&watch_path);
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let stamp = config_stamp(&watch_path);
                if stamp == last {
                    continue;
                }
                last = stamp;
                match Config::load_from(&watch_path) {
                    Ok(new) => {
                        if let Err(err) = new.validate() {
                            warn!("config reload ignored: {err:#}");
                            continue;
                        }
                        let restart = watch_app.apply_config(new);
                        if restart.is_empty() {
                            info!("config reloaded");
                        } else {
                            info!(
                                "config reloaded; restart required for: {}",
                                restart.join(", ")
                            );
                        }
                    }
                    Err(err) => warn!("config reload failed: {err:#}"),
                }
            }
        });
    }

    let listener_app = app.clone();
    tokio::spawn(async move {
        if let Err(err) = run_listener(listener_app).await {
            error!("listener stopped: {err:#}");
        }
    });

    let event_app = app.clone();
    tokio::spawn(async move {
        while let Some(local) = local_rx.recv().await {
            let cfg = event_app.config();
            let event = event_from_local(&cfg, local);
            event_app.record_local_event(&event, &cfg);
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

fn config_stamp(path: &Path) -> Option<(u128, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((modified, meta.len()))
}

fn init_logging(level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level.to_string()));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
