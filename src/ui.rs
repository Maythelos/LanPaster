slint::include_modules!();

use crate::{config::Config, instance, pairing::PairingManager};
use anyhow::{Context, Result};
use std::{path::PathBuf, rc::Rc, time::Duration};

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
    #[cfg(target_os = "macos")]
    make_regular_app();

    let window = SettingsWindow::new().context("create settings window")?;
    window
        .window()
        .set_size(slint::WindowSize::Logical(slint::LogicalSize::new(
            800.0, 640.0,
        )));
    let trusted_path = config_path.with_extension("trusted.toml");
    let pairing = Rc::new(PairingManager::new(&trusted_path)?);
    refresh_devices(&window, &pairing);
    fill_form(&window, &config);
    window.set_panel(match panel {
        Panel::Connection => 0,
        Panel::Local => 1,
    });
    window.set_app_version(env!("CARGO_PKG_VERSION").into());
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
        window.on_save_connection(move |form| {
            let result = save_connection(&path, &base, &form);
            if let Some(window) = window_weak.upgrade() {
                apply_status(
                    &window,
                    result,
                    "已保存；端口/mDNS/节点需重启，设备名已热加载",
                );
            }
        });
    }

    {
        let base = config.clone();
        let path = config_path.clone();
        let window_weak = window.as_weak();
        window.on_save_local(move |form| {
            let result = save_local(&path, &base, &form);
            if let Some(window) = window_weak.upgrade() {
                apply_status(&window, result, "已保存并热加载");
            }
        });
    }

    {
        let window_weak = window.as_weak();
        let pairing = pairing.clone();
        window.on_forget_device(move |id| {
            let id = id.to_string();
            let name = pairing
                .trusted_list()
                .into_iter()
                .find(|peer| peer.device_id == id)
                .map(|peer| peer.name)
                .unwrap_or_else(|| id.clone());
            let result = pairing.untrust(&id);
            if let Some(window) = window_weak.upgrade() {
                match result {
                    Ok(true) => {
                        refresh_devices(&window, &pairing);
                        set_status(&window, format!("已移除 {name}"), true);
                    }
                    Ok(false) => set_status(&window, "设备不存在".to_string(), false),
                    Err(err) => set_status(&window, format!("移除失败：{err:#}"), false),
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        let pairing = pairing.clone();
        window.on_forget_all_devices(move || {
            let result = pairing.untrust_all();
            if let Some(window) = window_weak.upgrade() {
                match result {
                    Ok(count) => {
                        refresh_devices(&window, &pairing);
                        set_status(&window, format!("已清空 {count} 台信任设备"), true);
                    }
                    Err(err) => set_status(&window, format!("清空失败：{err:#}"), false),
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        let pairing = pairing.clone();
        window.on_allow_device(move |id| {
            let id = id.to_string();
            let name = pairing
                .revoked_list()
                .into_iter()
                .find(|peer| peer.device_id == id)
                .map(|peer| peer.name)
                .unwrap_or_else(|| id.clone());
            let result = pairing.allow(&id);
            if let Some(window) = window_weak.upgrade() {
                match result {
                    Ok(true) => {
                        refresh_devices(&window, &pairing);
                        set_status(&window, format!("已恢复 {name}，重新连接时需再次配对"), true);
                    }
                    Ok(false) => set_status(&window, "设备不存在".to_string(), false),
                    Err(err) => set_status(&window, format!("恢复失败：{err:#}"), false),
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        let pairing = pairing.clone();
        window.on_allow_all_devices(move || {
            let result = pairing.allow_all();
            if let Some(window) = window_weak.upgrade() {
                match result {
                    Ok(count) => {
                        refresh_devices(&window, &pairing);
                        set_status(&window, format!("已恢复 {count} 台设备"), true);
                    }
                    Err(err) => set_status(&window, format!("恢复失败：{err:#}"), false),
                }
            }
        });
    }

    // The settings window is a separate process. When it was opened from the
    // tray, close it as soon as the tray daemon releases the per-config lock.
    // A standalone `lanpaster ui` (no daemon running) stays usable.
    let daemon_timer = {
        let from_tray = std::env::var_os("LANPASTER_TRAY_UI").is_some();
        let daemon_running = matches!(instance::acquire(&config_path), Ok(None));
        if from_tray || daemon_running {
            let timer = slint::Timer::default();
            let path = config_path.clone();
            timer.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(500),
                move || {
                    if let Ok(Some(lock)) = instance::acquire(&path) {
                        drop(lock);
                        let _ = slint::quit_event_loop();
                    }
                },
            );
            Some(timer)
        } else {
            None
        }
    };

    // Pick up trust changes made by the daemon (for example when the peer
    // removes this device) without reopening the window.
    let device_timer = {
        let window_weak = window.as_weak();
        let pairing = pairing.clone();
        let mut last_key = String::new();
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_secs(1),
            move || {
                let key = device_list_key(&pairing);
                if key == last_key {
                    return;
                }
                last_key = key;
                if let Some(window) = window_weak.upgrade() {
                    refresh_devices(&window, &pairing);
                }
            },
        );
        timer
    };

    window.show().context("show settings window")?;
    slint::run_event_loop().context("slint event loop")?;
    drop(daemon_timer);
    drop(device_timer);
    Ok(())
}

fn apply_status(window: &SettingsWindow, result: Result<()>, success: &str) {
    match result {
        Ok(()) => {
            window.set_status_error(false);
            window.set_status_success(true);
            window.set_status(success.into());
        }
        Err(err) => {
            window.set_status_error(true);
            window.set_status_success(false);
            window.set_status(format!("保存失败：{err:#}").into());
        }
    }
}

fn set_status(window: &SettingsWindow, text: String, success: bool) {
    window.set_status_error(!success);
    window.set_status_success(success);
    window.set_status(text.into());
}

fn device_list_key(pairing: &PairingManager) -> String {
    let mut parts: Vec<String> = pairing
        .trusted_list()
        .into_iter()
        .map(|p| format!("t\u{1f}{}\u{1f}{}\u{1f}{}", p.device_id, p.name, p.public_key))
        .collect();
    parts.extend(
        pairing
            .revoked_list()
            .into_iter()
            .map(|p| format!("r\u{1f}{}\u{1f}{}\u{1f}{}", p.device_id, p.name, p.public_key)),
    );
    parts.sort();
    parts.join("\u{1e}")
}

fn refresh_devices(window: &SettingsWindow, pairing: &PairingManager) {
    let mut peers = pairing.trusted_list();
    peers.sort_by_key(|peer| peer.name.to_lowercase());
    let devices: Vec<TrustedDevice> = peers
        .into_iter()
        .map(|peer| TrustedDevice {
            device_id: peer.device_id.into(),
            name: peer.name.into(),
            fingerprint: PairingManager::fingerprint(
                &hex::decode(&peer.public_key).unwrap_or_default(),
            )
            .into(),
        })
        .collect();
    window.set_trusted_devices(slint::ModelRc::new(slint::VecModel::from(devices)));

    let mut revoked = pairing.revoked_list();
    revoked.sort_by_key(|peer| peer.name.to_lowercase());
    let removed: Vec<RevokedDevice> = revoked
        .into_iter()
        .map(|peer| RevokedDevice {
            device_id: peer.device_id.into(),
            name: peer.name.into(),
            fingerprint: PairingManager::fingerprint(
                &hex::decode(&peer.public_key).unwrap_or_default(),
            )
            .into(),
        })
        .collect();
    window.set_revoked_devices(slint::ModelRc::new(slint::VecModel::from(removed)));
}

fn fill_form(window: &SettingsWindow, config: &Config) {
    window.set_device_name(config.device_name.clone().into());
    window.set_fingerprint(
        config
            .public_key_bytes()
            .map(|key| PairingManager::fingerprint(&key))
            .unwrap_or_default()
            .into(),
    );
    window.set_listen_port(config.listen_port.to_string().into());
    window.set_mdns_enabled(config.mdns_enabled);
    window.set_peers(config.peers.join(", ").into());
    window.set_poll_ms(config.poll_interval_ms.to_string().into());
    window.set_max_text_kb(config.max_text_kb.to_string().into());
    window.set_max_file_mb(config.max_file_mb.to_string().into());
    window.set_max_batch_mb(config.max_batch_mb.to_string().into());
    window.set_max_files(config.max_files.to_string().into());
    window.set_history_minutes(config.history_minutes.to_string().into());
    window.set_history_limit(config.history_limit.to_string().into());
    window.set_file_sync(config.file_sync);
    window.set_sound_enabled(config.sound_enabled);
}

fn save_connection(path: &std::path::Path, fallback: &Config, form: &ConnectionForm) -> Result<()> {
    let mut cfg = Config::load_from(path).unwrap_or_else(|_| fallback.clone());
    cfg.device_name = form.device_name.trim().to_string();
    if cfg.device_name.is_empty() {
        anyhow::bail!("设备名称不能为空");
    }
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
    cfg.history_minutes = parse_u64(&form.history_minutes, "历史保留时长")?;
    cfg.history_limit = parse_usize(&form.history_limit, "历史条目上限")?;
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
            listen_port: "40000".into(),
            mdns_enabled: false,
            peers: "127.0.0.1:40001, 127.0.0.1:40002".into(),
        };
        let path = std::env::temp_dir().join(format!(
            "lanpaster-connection-form-{}.toml",
            crate::config::random_hex(4)
        ));
        let mut initial = base.clone();
        let (private, public) = crate::crypto::generate_identity();
        initial.private_key = private;
        initial.public_key = public;
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
            history_minutes: "10".into(),
            history_limit: "20".into(),
            file_sync: true,
            sound_enabled: false,
        };
        let path = std::env::temp_dir().join(format!(
            "lanpaster-local-form-{}.toml",
            crate::config::random_hex(4)
        ));
        let mut initial = base.clone();
        let (private, public) = crate::crypto::generate_identity();
        initial.private_key = private;
        initial.public_key = public;
        initial.save(&path).unwrap();
        save_local(&path, &initial, &form).unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.max_file_mb, 100);
        assert_eq!(cfg.history_minutes, 10);
        assert_eq!(cfg.history_limit, 20);
        assert!(!cfg.sound_enabled);
        let _ = std::fs::remove_file(path);
    }
}
