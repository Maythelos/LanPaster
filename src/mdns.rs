use crate::{config::Config, net::AppContext};
use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::{collections::HashMap, net::Ipv4Addr, sync::Arc};
use tracing::{debug, info, warn};

const SERVICE_TYPE: &str = "_lanpaster._tcp.local.";

pub fn spawn(config: Config, app: Arc<AppContext>, handle: tokio::runtime::Handle) {
    std::thread::Builder::new()
        .name("lanpaster-mdns".to_string())
        .spawn(move || {
            if let Err(err) = run(config, app, handle) {
                warn!("mDNS stopped: {err:#}");
            }
        })
        .ok();
}

fn run(config: Config, app: Arc<AppContext>, handle: tokio::runtime::Handle) -> Result<()> {
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

    let receiver = daemon.browse(SERVICE_TYPE).context("browse mDNS")?;
    loop {
        match receiver.recv() {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                if info.get_property_val_str("id") == Some(config.device_id.as_str()) {
                    continue;
                }
                let port = info.get_port();
                let name = info
                    .get_property_val_str("name")
                    .unwrap_or(info.get_fullname());
                for addr in info.get_addresses_v4() {
                    let addr = format!("{addr}:{port}");
                    debug!("discovered {name} at {addr}");
                    let app = app.clone();
                    handle.spawn(async move {
                        app.ensure_dial(addr).await;
                    });
                }
            }
            Ok(ServiceEvent::ServiceRemoved(_, fullname)) => {
                debug!("mDNS service removed: {fullname}");
            }
            Ok(ServiceEvent::SearchStarted(_))
            | Ok(ServiceEvent::ServiceFound(_, _))
            | Ok(ServiceEvent::SearchStopped(_)) => {}
            Err(err) => {
                debug!("mDNS receive error: {err}");
                break;
            }
            _ => {}
        }
    }
    Ok(())
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
