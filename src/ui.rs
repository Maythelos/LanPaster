slint::include_modules!();

use crate::config::Config;
use anyhow::{Context, Result};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Connection,
    Local,
}

impl Panel {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "connection" | "connect" | "network" => Some(Self::Connection),
            "local" | "preferences" | "clipboard" => Some(Self::Local),
            _ => None,
        }
    }
}

pub fn run_config_window(config: Config, config_path: PathBuf, panel: Panel) -> Result<()> {
    match panel {
        Panel::Connection => run_connection_window(config, config_path),
        Panel::Local => run_local_window(config, config_path),
    }
}

fn run_connection_window(config: Config, config_path: PathBuf) -> Result<()> {
    #[cfg(target_os = "macos")]
    make_regular_app();
    let window = ConnectionWindow::new().context("create connection window")?;
    window.set_device_name(config.device_name.clone().into());
    window.set_psk(config.psk.clone().into());
    window.set_listen_port(config.listen_port.to_string().into());
    window.set_mdns_enabled(config.mdns_enabled);
    window.set_peers(config.peers.join(", ").into());
    window.set_status("".into());
    window.set_status_error(false);
    window.set_status_success(false);

    {
        let window_weak = window.as_weak();
        window.on_close_window(move || {
            if let Some(window) = window_weak.upgrade() {
                let _ = window.hide();
            }
            let _ = slint::quit_event_loop();
        });
    }
    window.window().on_close_requested(|| {
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });

    {
        let base = config.clone();
        let path = config_path.clone();
        let window_weak = window.as_weak();
        window.on_save(move |form| {
            let result = save_connection(&path, &base, &form);
            if let Some(window) = window_weak.upgrade() {
                match result {
                    Ok(()) => {
                        window.set_status_error(false);
                        window.set_status_success(true);
                        window.set_status(
                            "已保存；端口/PSK/mDNS/节点需重启，设备名已热加载".into(),
                        );
                    }
                    Err(err) => {
                        window.set_status_error(true);
                        window.set_status_success(false);
                        window.set_status(format!("保存失败：{err:#}").into());
                    }
                }
            }
        });
    }

    window.show().context("show connection window")?;
    slint::run_event_loop().context("slint event loop")?;
    Ok(())
}

fn run_local_window(config: Config, config_path: PathBuf) -> Result<()> {
    #[cfg(target_os = "macos")]
    make_regular_app();
    let window = LocalWindow::new().context("create local window")?;
    window.set_poll_ms(config.poll_interval_ms.to_string().into());
    window.set_max_text_kb(config.max_text_kb.to_string().into());
    window.set_max_file_mb(config.max_file_mb.to_string().into());
    window.set_max_batch_mb(config.max_batch_mb.to_string().into());
    window.set_max_files(config.max_files.to_string().into());
    window.set_file_sync(config.file_sync);
    window.set_sound_enabled(config.sound_enabled);
    window.set_status("".into());
    window.set_status_error(false);
    window.set_status_success(false);

    {
        let window_weak = window.as_weak();
        window.on_close_window(move || {
            if let Some(window) = window_weak.upgrade() {
                let _ = window.hide();
            }
            let _ = slint::quit_event_loop();
        });
    }
    window.window().on_close_requested(|| {
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });

    {
        let base = config.clone();
        let path = config_path.clone();
        let window_weak = window.as_weak();
        window.on_save(move |form| {
            let result = save_local(&path, &base, &form);
            if let Some(window) = window_weak.upgrade() {
                match result {
                    Ok(()) => {
                        window.set_status_error(false);
                        window.set_status_success(true);
                        window.set_status("已保存并热加载".into());
                    }
                    Err(err) => {
                        window.set_status_error(true);
                        window.set_status_success(false);
                        window.set_status(format!("保存失败：{err:#}").into());
                    }
                }
            }
        });
    }

    window.show().context("show local window")?;
    slint::run_event_loop().context("slint event loop")?;
    Ok(())
}

fn save_connection(path: &std::path::Path, fallback: &Config, form: &ConnectionForm) -> Result<()> {
    let mut cfg = Config::load_from(path).unwrap_or_else(|_| fallback.clone());
    cfg.device_name = form.device_name.trim().to_string();
    if cfg.device_name.is_empty() {
        anyhow::bail!("设备名称不能为空");
    }
    cfg.psk = form.psk.trim().to_string();
    cfg.listen_port = parse_u16(&form.listen_port, "监听端口")?;
    cfg.mdns_enabled = form.mdns_enabled;
    cfg.peers = parse_peers(&form.peers);
    cfg.validate()?;
    cfg.save(path)
}

fn save_local(path: &std::path::Path, fallback: &Config, form: &LocalForm) -> Result<()> {
    let mut cfg = Config::load_from(path).unwrap_or_else(|_| fallback.clone());
    cfg.poll_interval_ms = parse_u64(&form.poll_ms, "轮询间隔")?;
    if cfg.poll_interval_ms < 100 {
        anyhow::bail!("轮询间隔不能小于 100ms");
    }
    cfg.max_text_kb = parse_u64(&form.max_text_kb, "文本上限")?;
    cfg.max_file_mb = parse_u64(&form.max_file_mb, "单文件 / 图片上限")?;
    cfg.max_batch_mb = parse_u64(&form.max_batch_mb, "单次总量上限")?;
    cfg.max_files = parse_usize(&form.max_files, "文件数量上限")?;
    cfg.file_sync = form.file_sync;
    cfg.sound_enabled = form.sound_enabled;
    cfg.validate()?;
    cfg.save(path)
}

#[cfg(target_os = "macos")]
fn make_regular_app() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.activate();
}

fn parse_u64(value: &str, name: &str) -> Result<u64> {
    value
        .trim()
        .parse::<u64>()
        .with_context(|| format!("{name}必须是整数"))
}

fn parse_u16(value: &str, name: &str) -> Result<u16> {
    value
        .trim()
        .parse::<u16>()
        .with_context(|| format!("{name}必须是 1-65535 的整数"))
}

fn parse_usize(value: &str, name: &str) -> Result<usize> {
    value
        .trim()
        .parse::<usize>()
        .with_context(|| format!("{name}必须是非负整数"))
}

fn parse_peers(value: &str) -> Vec<String> {
    let mut peers = Vec::new();
    for item in value.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
        let item = item.trim();
        if !item.is_empty() && !peers.iter().any(|p: &String| p == item) {
            peers.push(item.to_string());
        }
    }
    peers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_form_parses() {
        let base = Config::default();
        let form = ConnectionForm {
            device_name: "test-mac".into(),
            psk: "ab".repeat(32).into(),
            listen_port: "40000".into(),
            mdns_enabled: false,
            peers: "127.0.0.1:40001, 127.0.0.1:40002".into(),
        };
        let path = std::env::temp_dir().join(format!(
            "lanpaster-connection-form-{}.toml",
            crate::config::random_hex(4)
        ));
        let mut initial = base.clone();
        initial.psk = form.psk.to_string();
        initial.save(&path).unwrap();
        save_connection(&path, &initial, &form).unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.device_name, "test-mac");
        assert_eq!(cfg.listen_port, 40000);
        assert_eq!(cfg.peers.len(), 2);
        assert!(!cfg.mdns_enabled);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn local_form_parses() {
        let base = Config::default();
        let form = LocalForm {
            poll_ms: "500".into(),
            max_text_kb: "1024".into(),
            max_file_mb: "100".into(),
            max_batch_mb: "200".into(),
            max_files: "50".into(),
            file_sync: true,
            sound_enabled: false,
        };
        let path = std::env::temp_dir().join(format!(
            "lanpaster-local-form-{}.toml",
            crate::config::random_hex(4)
        ));
        let mut initial = base.clone();
        initial.psk = "ab".repeat(32);
        initial.save(&path).unwrap();
        save_local(&path, &initial, &form).unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.max_file_mb, 100);
        assert!(!cfg.sound_enabled);
        let _ = std::fs::remove_file(path);
    }
}
