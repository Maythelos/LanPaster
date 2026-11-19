use crate::{
    config::Config,
    net::{AppContext, DiscoveredPeer},
};
use anyhow::{bail, Context, Result};
use mdns_sd::{RecvTimeoutError, ServiceDaemon, ServiceEvent, ServiceInfo};
use std::{
    collections::HashMap,
    net::Ipv4Addr,
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};
use tracing::{debug, info, warn};

const SERVICE_TYPE: &str = "_lanpaster._tcp.local.";

pub fn spawn(config: Config, app: Arc<AppContext>) {
    let (scan_tx, scan_rx) = mpsc::channel();
    app.set_scan_control(scan_tx);
    std::thread::Builder::new()
        .name("lanpaster-mdns".to_string())
        .spawn(move || loop {
            if let Err(err) = run(config.clone(), app.clone(), &scan_rx) {
                warn!("mDNS stopped: {err:#}");
                app.clear_discovered();
                std::thread::sleep(Duration::from_secs(2));
            } else {
                break;
            }
        })
        .ok();
}

fn run(config: Config, app: Arc<AppContext>, scan_rx: &mpsc::Receiver<()>) -> Result<()> {
    let daemon = ServiceDaemon::new().context("create mDNS daemon")?;
    let ip = local_ipv4().unwrap_or_else(|| "127.0.0.1".to_string());
    let instance = format!(
        "{}-{}",
        sanitize_instance(&config.device_name),
        &config.device_id[..config.device_id.len().min(6)]
    );
    let host = format!("{}.local.", sanitize_instance(&config.device_name));
    let mut props = HashMap::new();
    props.insert("id".to_string(), config.device_id.clone());
    props.insert("name".to_string(), config.device_name.clone());
    props.insert("proto".to_string(), crate::protocol::PROTOCOL_VERSION.to_string());

    let service = ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &host,
        ip.as_str(),
        config.listen_port,
        Some(props),
    )
    .context("build mDNS service info")?;
    daemon.register(service).context("register mDNS service")?;
    info!("mDNS registered {SERVICE_TYPE} on {ip}:{}", config.listen_port);

    let mut receiver = daemon.browse(SERVICE_TYPE).context("browse mDNS")?;
    loop {
        // A manual scan sends a fresh query. Discovery results are only stored;
        // connecting stays an explicit user action.
        while scan_rx.try_recv().is_ok() {
            info!("mDNS rescan requested");
            app.clear_discovered();
            let _ = daemon.stop_browse(SERVICE_TYPE);
            receiver = daemon.browse(SERVICE_TYPE).context("browse mDNS")?;
        }

        match receiver.recv_timeout(Duration::from_millis(500)) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                handle_resolved(&config, &app, &info);
            }
            Ok(ServiceEvent::ServiceRemoved(_, fullname)) => {
                debug!("mDNS service removed: {fullname}");
                app.remove_discovered(&fullname);
            }
            Ok(ServiceEvent::SearchStarted(_))
            | Ok(ServiceEvent::ServiceFound(_, _))
            | Ok(ServiceEvent::SearchStopped(_)) => {}
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                bail!("mDNS browse channel closed");
            }
        }
    }
}

fn handle_resolved(
    config: &Config,
    app: &Arc<AppContext>,
    info: &mdns_sd::ResolvedService,
) {
    let Some(device_id) = info.get_property_val_str("id") else {
        debug!("mDNS service without device id: {}", info.get_fullname());
        return;
    };
    if device_id == config.device_id.as_str() {
        return;
    }
    let device_name = info
        .get_property_val_str("name")
        .unwrap_or_else(|| info.get_fullname())
        .to_string();
    let protocol = info
        .get_property_val_str("proto")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let port = info.get_port();
    let mut addrs: Vec<Ipv4Addr> = info.get_addresses_v4().into_iter().collect();
    addrs.sort_by_key(|addr| {
        (addr.is_link_local(), addr.is_loopback(), !addr.is_private())
    });
    let Some(ip) = addrs.first() else {
        debug!("mDNS resolved {device_name} without an IPv4 address");
        return;
    };
    let addr = format!("{ip}:{port}");
    info!(
        "mDNS discovered {device_name} at {addr} (protocol {protocol})"
    );
    app.add_discovered(DiscoveredPeer {
        device_id: device_id.to_string(),
        device_name,
        addr,
        protocol,
        fullname: info.get_fullname().to_string(),
        seen_at: Instant::now(),
    });
}

fn local_ipv4() -> Option<String> {
    let mut best: Option<(u8, Ipv4Addr)> = None;
    for iface in if_addrs::get_if_addrs().ok()? {
        let ip = match iface.addr {
            if_addrs::IfAddr::V4(v4) => v4.ip,
            _ => continue,
        };
        if ip.is_loopback() || ip.is_link_local() {
            continue;
        }
        let name = iface.name.to_ascii_lowercase();
        if name.starts_with("utun") || name.starts_with("bridge") || name.starts_with("ap") {
            continue;
        }
        let mut score = 0u8;
        if name.starts_with("en") || name.starts_with("eth") || name.starts_with("wlan") {
            score += 4;
        }
        if ip.is_private() {
            score += 2;
        }
        if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
            best = Some((score, ip));
        }
    }
    best.map(|(_, ip)| ip.to_string())
}

fn sanitize_instance(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    if name.is_empty() {
        "lanpaster".to_string()
    } else {
        name
    }
}
