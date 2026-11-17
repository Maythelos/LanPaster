use anyhow::{Context, Result};
use std::fs;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Stdio;
use tracing::info;
#[cfg(target_os = "macos")]
use tracing::warn;

#[cfg(target_os = "macos")]
pub fn is_enabled() -> bool {
    macos_plist_path().exists()
}

#[cfg(target_os = "macos")]
pub fn set(enabled: bool) -> Result<()> {
    let path = macos_plist_path();
    if enabled {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let exe = std::env::current_exe().context("current executable")?;
        let label = "com.lanpaster.agent";
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
</dict>
</plist>
"#,
            xml_escape(&exe.to_string_lossy())
        );
        fs::write(&path, plist)?;
        let uid = current_uid()?;
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &format!("gui/{uid}/{label}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match std::process::Command::new("launchctl")
            .args(["bootstrap", &format!("gui/{uid}"), &path.to_string_lossy()])
            .status()
        {
            Ok(status) if status.success() => info!("autostart enabled"),
            Ok(status) => warn!("launchctl bootstrap returned {status}"),
            Err(err) => warn!("launchctl bootstrap failed: {err}"),
        }
    } else {
        let uid = current_uid()?;
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &format!("gui/{uid}/com.lanpaster.agent")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if path.exists() {
            fs::remove_file(&path)?;
        }
        info!("autostart disabled");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn is_enabled() -> bool {
    linux_desktop_path().exists()
}

#[cfg(target_os = "linux")]
pub fn set(enabled: bool) -> Result<()> {
    let path = linux_desktop_path();
    if enabled {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let exe = std::env::current_exe().context("current executable")?;
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName=LanPaster\nExec=\"{}\" run\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
            exe.to_string_lossy()
        );
        fs::write(&path, desktop)?;
        info!("autostart enabled");
    } else {
        if path.exists() {
            fs::remove_file(&path)?;
        }
        info!("autostart disabled");
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn is_enabled() -> bool {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
        .and_then(|key| key.get_value::<String, _>("LanPaster"))
        .is_ok()
}

#[cfg(target_os = "windows")]
pub fn set(enabled: bool) -> Result<()> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")?;
    if enabled {
        let exe = std::env::current_exe().context("current executable")?;
        key.set_value("LanPaster", &format!("\"{}\" run", exe.display()))?;
        info!("autostart enabled");
    } else {
        let _ = key.delete_value("LanPaster");
        info!("autostart disabled");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_plist_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/LaunchAgents/com.lanpaster.agent.plist")
}

#[cfg(target_os = "linux")]
fn linux_desktop_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("autostart/lanpaster.desktop")
}

#[cfg(target_os = "macos")]
fn current_uid() -> Result<String> {
    let output = std::process::Command::new("id")
        .arg("-u")
        .output()
        .context("run id -u")?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
