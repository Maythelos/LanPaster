slint::include_modules!();

use crate::config::Config;
use anyhow::{Context, Result};
use std::path::PathBuf;

pub fn run_config_window(config: Config, config_path: PathBuf) -> Result<()> {
    #[cfg(target_os = "macos")]
    make_regular_app();
    let window = ConfigWindow::new().context("create config window")?;
    fill_form(&window, &config);
    window.set_status("阈值/文件同步/提示音/轮询会热加载；端口/密钥/mDNS/节点需要重启。".into());

    {
        let window_weak = window.as_weak();
        window.on_close_window(move || {
            if let Some(window) = window_weak.upgrade() {
                let _ = window.hide();
            }
            let _ = slint::quit_event_loop();
        });
    }

    // The OS title-bar close button should terminate this short-lived process,
    // not just hide the window.
    window.window().on_close_requested(|| {
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });

    {
        let mut current = config.clone();
        let path = config_path.clone();
        let window_weak = window.as_weak();
        window.on_save(move |form| {
            let next = match form_to_config(&current, &form) {
                Ok(next) => next,
                Err(err) => {
                    if let Some(window) = window_weak.upgrade() {
                        window.set_status(format!("配置无效：{err}").into());
                    }
                    return;
                }
            };
            if let Err(err) = next.validate() {
                if let Some(window) = window_weak.upgrade() {
                    window.set_status(format!("配置无效：{err}").into());
                }
                return;
            }
            match next.save(&path) {
                Ok(()) => {
                    current = next;
                    if let Some(window) = window_weak.upgrade() {
                        window.set_status("已保存；热加载配置已生效，网络/密钥/节点变更需重启".into());
                    }
                }
                Err(err) => {
                    if let Some(window) = window_weak.upgrade() {
                        window.set_status(format!("保存失败：{err:#}").into());
                    }
                }
            }
        });
    }

    window.show().context("show config window")?;
    slint::run_event_loop().context("slint event loop")?;
    Ok(())
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

fn fill_form(window: &ConfigWindow, config: &Config) {
    window.set_device_name(config.device_name.clone().into());
    window.set_psk(config.psk.clone().into());
    window.set_listen_port(config.listen_port.to_string().into());
    window.set_poll_ms(config.poll_interval_ms.to_string().into());
    window.set_max_text_kb(config.max_text_kb.to_string().into());
    window.set_max_image_mb(config.max_image_mb.to_string().into());
    window.set_max_file_mb(config.max_file_mb.to_string().into());
    window.set_max_batch_mb(config.max_batch_mb.to_string().into());
    window.set_max_files(config.max_files.to_string().into());
    window.set_max_image_pixels(config.max_image_pixels.to_string().into());
    window.set_file_sync(config.file_sync);
    window.set_sound_enabled(config.sound_enabled);
    window.set_mdns_enabled(config.mdns_enabled);
    window.set_peers(config.peers.join(", ").into());
}

fn form_to_config(base: &Config, form: &ConfigForm) -> Result<Config> {
    let mut cfg = base.clone();
    cfg.device_name = form.device_name.trim().to_string();
    cfg.psk = form.psk.trim().to_string();
    cfg.listen_port = parse_u16(&form.listen_port, "监听端口")?;
    cfg.poll_interval_ms = parse_u64(&form.poll_ms, "轮询间隔")?;
    cfg.max_text_kb = parse_u64(&form.max_text_kb, "文本上限")?;
    cfg.max_image_mb = parse_u64(&form.max_image_mb, "图片上限")?;
    cfg.max_file_mb = parse_u64(&form.max_file_mb, "单文件上限")?;
    cfg.max_batch_mb = parse_u64(&form.max_batch_mb, "单次总量上限")?;
    cfg.max_files = parse_usize(&form.max_files, "文件数量上限")?;
    cfg.max_image_pixels = parse_u64(&form.max_image_pixels, "图片像素上限")?;
    cfg.file_sync = form.file_sync;
    cfg.sound_enabled = form.sound_enabled;
    cfg.mdns_enabled = form.mdns_enabled;
    cfg.peers = parse_peers(&form.peers);
    if cfg.poll_interval_ms < 100 {
        anyhow::bail!("轮询间隔不能小于 100ms");
    }
    Ok(cfg)
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
    fn form_parses_into_config() {
        let base = Config::default();
        let form = ConfigForm {
            device_name: "test-mac".into(),
            psk: "ab".repeat(32).into(),
            listen_port: "40000".into(),
            poll_ms: "500".into(),
            max_text_kb: "1024".into(),
            max_image_mb: "20".into(),
            max_file_mb: "100".into(),
            max_batch_mb: "200".into(),
            max_files: "50".into(),
            max_image_pixels: "50000000".into(),
            file_sync: true,
            sound_enabled: true,
            mdns_enabled: false,
            peers: "127.0.0.1:40001, 127.0.0.1:40002".into(),
        };
        let cfg = form_to_config(&base, &form).unwrap();
        assert_eq!(cfg.device_name, "test-mac");
        assert_eq!(cfg.listen_port, 40000);
        assert_eq!(cfg.max_file_mb, 100);
        assert_eq!(cfg.peers.len(), 2);
        assert!(!cfg.mdns_enabled);
    }
}
