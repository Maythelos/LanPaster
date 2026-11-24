//! GitHub Releases 自更新。
//!
//! 流程：查询 latest release -> 选择平台安装包 -> 下载并校验
//! SHA256SUMS.txt -> 解压到缓存目录 -> 启动一个等待当前进程退出的
//! 小脚本 -> 替换安装位置并重新启动。
//!
//! 更新包和校验文件来自同一个 GitHub Release，校验只能保证下载完整，
//! 不能替代代码签名。只在用户从托盘菜单主动检查时联网。

use crate::config;
use anyhow::{bail, Context, Result};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    env,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
    time::{Duration, Instant},
};
use tracing::info;

const RELEASE_API: &str = "https://api.github.com/repos/Maythelos/LanPaster/releases/latest";
const USER_AGENT: &str = concat!("LanPaster/", env!("CARGO_PKG_VERSION"));
const MAX_ASSET_BYTES: u64 = 256 * 1024 * 1024;
const MAX_TEXT_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub version: Version,
    pub page_url: String,
    pub asset_name: String,
    pub asset_url: String,
    pub asset_size: u64,
    pub sums_url: Option<String>,
}

#[derive(Debug)]
pub struct StagedUpdate {
    pub version: Version,
    pub payload: PathBuf,
}

#[derive(Debug, Clone)]
pub enum UpdateEvent {
    Checking,
    UpToDate,
    Available(ReleaseInfo),
    Downloading(String),
    Installing(String),
    Failed(String),
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    Macos,
    Windows,
    Linux,
}

pub fn current_version() -> Result<Version> {
    Version::parse(env!("CARGO_PKG_VERSION")).context("parse current version")
}

pub fn check_for_update() -> Result<Option<ReleaseInfo>> {
    let api = env::var("LANPASTER_UPDATE_API").unwrap_or_else(|_| RELEASE_API.to_string());
    let body = get_text(&api, MAX_TEXT_BYTES).context("查询 GitHub Release")?;
    parse_release(&body, &current_version()?)
}

fn parse_release(body: &str, current: &Version) -> Result<Option<ReleaseInfo>> {
    let release: GhRelease = serde_json::from_str(body).context("解析 GitHub Release 响应失败")?;
    let version = parse_version_tag(&release.tag_name)?;
    if version <= *current {
        return Ok(None);
    }

    let platform = platform();
    let asset = release
        .assets
        .iter()
        .filter_map(|asset| asset_rank(&asset.name, platform).map(|rank| (rank, asset)))
        .min_by(|(left_rank, left), (right_rank, right)| {
            left_rank
                .cmp(right_rank)
                .then_with(|| left.name.cmp(&right.name))
        })
        .map(|(_, asset)| asset)
        .with_context(|| format!("版本 {} 没有适用于当前平台的安装包", release.tag_name))?;

    if asset.size > MAX_ASSET_BYTES {
        bail!("更新包过大（{} 字节），已拒绝下载", asset.size);
    }

    let sums = release
        .assets
        .iter()
        .find(|asset| asset.name == "SHA256SUMS.txt")
        .with_context(|| {
            format!(
                "版本 {} 缺少 SHA256SUMS.txt，已拒绝自动更新",
                release.tag_name
            )
        })?;

    Ok(Some(ReleaseInfo {
        version,
        page_url: release.html_url,
        asset_name: asset.name.clone(),
        asset_url: asset.browser_download_url.clone(),
        asset_size: asset.size,
        sums_url: Some(sums.browser_download_url.clone()),
    }))
}

fn parse_version_tag(tag: &str) -> Result<Version> {
    let raw = tag.trim();
    let raw = raw
        .strip_prefix('v')
        .or_else(|| raw.strip_prefix('V'))
        .unwrap_or(raw);
    Version::parse(raw).with_context(|| format!("无法解析版本号 {tag:?}"))
}

fn platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Linux
    }
}

/// 返回越小优先级越高；`None` 表示不是当前平台的包。
fn asset_rank(name: &str, platform: Platform) -> Option<u8> {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    match platform {
        Platform::Macos => {
            if lower.ends_with("-macos-universal.zip") {
                Some(0)
            } else {
                None
            }
        }
        Platform::Windows => {
            if lower.ends_with("-windows-x86_64.zip") {
                Some(0)
            } else {
                None
            }
        }
        Platform::Linux => {
            if lower.ends_with("-linux-x86_64.tar.gz") {
                Some(0)
            } else if lower == "lanpaster-linux-amd64" {
                Some(1)
            } else {
                None
            }
        }
    }
}

fn get_text(url: &str, limit: u64) -> Result<String> {
    let response = agent()
        .get(url)
        .set("Accept", "application/vnd.github+json")
        .call()
        .with_context(|| format!("请求 {url} 失败"))?;
    let mut reader = response.into_reader().take(limit + 1);
    let mut body = Vec::new();
    reader.read_to_end(&mut body)?;
    if body.len() as u64 > limit {
        bail!("响应内容超过限制");
    }
    String::from_utf8(body).context("响应不是 UTF-8 文本")
}

fn download_asset(url: &str, dest: &Path) -> Result<String> {
    let response = agent()
        .get(url)
        .call()
        .with_context(|| format!("下载 {url} 失败"))?;
    if let Some(length) = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok())
    {
        if length > MAX_ASSET_BYTES {
            bail!("更新包过大（{length} 字节），已拒绝下载");
        }
    }

    let mut reader = response.into_reader();
    let mut file =
        File::create(dest).with_context(|| format!("创建下载文件 {}", dest.display()))?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > MAX_ASSET_BYTES {
            bail!("更新包超过大小限制");
        }
        hasher.update(&buf[..read]);
        file.write_all(&buf[..read])?;
    }
    file.sync_all()?;
    if total == 0 {
        bail!("下载内容为空");
    }
    Ok(hex::encode(hasher.finalize()))
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(120))
        .timeout_write(Duration::from_secs(30))
        .user_agent(USER_AGENT)
        .build()
}

/// 下载、校验并解压更新包。成功后返回可以直接交给安装脚本的路径。
pub fn download_and_stage(info: &ReleaseInfo) -> Result<StagedUpdate> {
    let dir = prepare_update_dir()?;
    let sums_url = info
        .sums_url
        .as_deref()
        .context("Release 缺少 SHA256SUMS.txt 地址")?;
    let sums = get_text(sums_url, MAX_TEXT_BYTES).context("下载 SHA256SUMS.txt")?;
    let expected = checksum_for(&sums, &info.asset_name)?;

    let archive = dir.join(&info.asset_name);
    let part = dir.join(format!("{}.part", info.asset_name));
    let actual = download_asset(&info.asset_url, &part)?;
    if actual != expected {
        let _ = fs::remove_file(&part);
        bail!("更新包校验失败：期望 {expected}，实际 {actual}；已丢弃下载文件");
    }
    if let Some(size) = (info.asset_size > 0).then_some(info.asset_size) {
        let downloaded = fs::metadata(&part)?.len();
        if downloaded != size {
            let _ = fs::remove_file(&part);
            bail!("更新包大小不符：期望 {size}，实际 {downloaded}");
        }
    }
    fs::rename(&part, &archive)?;

    let stage = dir.join(format!("stage-{}", info.version));
    if stage.exists() {
        fs::remove_dir_all(&stage)?;
    }
    fs::create_dir_all(&stage)?;
    if let Err(err) = extract_archive(&archive, &stage, platform()) {
        let _ = fs::remove_dir_all(&stage);
        let _ = fs::remove_file(&archive);
        return Err(err);
    }

    let payload = match platform() {
        Platform::Macos => find_app(&stage).context("更新包中没有找到 LanPaster.app")?,
        Platform::Windows => {
            find_file(&stage, "lanpaster.exe").context("更新包中没有找到 lanpaster.exe")?;
            stage.clone()
        }
        Platform::Linux => stage
            .join("usr/bin/lanpaster")
            .exists()
            .then(|| stage.join("usr/bin/lanpaster"))
            .or_else(|| find_file(&stage, "lanpaster"))
            .context("更新包中没有找到 lanpaster 可执行文件")?,
    };
    if !payload.exists() {
        bail!("解压后的更新文件不存在：{}", payload.display());
    }
    verify_staged_version(&payload, &info.version, platform())?;
    let _ = fs::remove_file(&archive);
    info!(
        "update {} staged at {} ({})",
        info.version,
        payload.display(),
        info.asset_name
    );
    Ok(StagedUpdate {
        version: info.version.clone(),
        payload,
    })
}

fn verify_staged_version(payload: &Path, expected: &Version, platform: Platform) -> Result<()> {
    let exe = match platform {
        Platform::Macos => payload.join("Contents/MacOS/lanpaster"),
        Platform::Linux => payload.to_path_buf(),
        Platform::Windows => find_file(payload, "lanpaster-console.exe")
            .or_else(|| find_file(payload, "lanpaster.exe"))
            .context("更新包中没有可用于版本检查的可执行文件")?,
    };
    if !exe.is_file() {
        bail!("更新包中的可执行文件不存在：{}", exe.display());
    }
    let _ = make_executable(&exe);
    let reported = run_version(&exe)?;
    if &reported != expected {
        bail!("更新包内程序版本 v{reported} 与 Release v{expected} 不一致，已拒绝安装");
    }
    info!("update package version verified: v{reported}");
    Ok(())
}

fn run_version(exe: &Path) -> Result<Version> {
    let mut command = Command::new(exe);
    command
        .arg("version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("运行更新包版本检查失败：{}", exe.display()))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("更新包版本检查超时：{}", exe.display());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child
        .wait_with_output()
        .with_context(|| format!("读取更新包版本失败：{}", exe.display()))?;
    if !output.status.success() {
        bail!("更新包版本检查失败：{}", output.status);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let reported = text
        .split_whitespace()
        .last()
        .and_then(|value| Version::parse(value.trim_start_matches('v')).ok())
        .with_context(|| format!("无法解析更新包版本输出：{}", text.trim()))?;
    Ok(reported)
}

fn checksum_for(sums: &str, name: &str) -> Result<String> {
    for line in sums.lines() {
        let mut parts = line.split_whitespace();
        let (Some(hash), Some(file)) = (parts.next(), parts.next()) else {
            continue;
        };
        if file.trim_start_matches('*') == name {
            if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                bail!("SHA256SUMS.txt 中 {name} 的校验值格式错误");
            }
            return Ok(hash.to_ascii_lowercase());
        }
    }
    bail!("SHA256SUMS.txt 中缺少 {name} 的校验值")
}

fn prepare_update_dir() -> Result<PathBuf> {
    let dir = config::cache_dir().join("updates");
    fs::create_dir_all(&dir).with_context(|| format!("创建更新缓存目录 {}", dir.display()))?;
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let stale = name.starts_with("stage-")
            || name.starts_with("apply-")
            || name.ends_with(".part")
            || name.ends_with(".zip")
            || name.ends_with(".tar.gz");
        if !stale {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            let _ = fs::remove_dir_all(path);
        } else {
            let _ = fs::remove_file(path);
        }
    }
    Ok(dir)
}

fn extract_archive(archive: &Path, stage: &Path, platform: Platform) -> Result<()> {
    match platform {
        Platform::Macos => run(
            Command::new("/usr/bin/ditto")
                .args(["-x", "-k"])
                .arg(archive)
                .arg(stage),
            "解压 macOS 更新包",
        ),
        Platform::Windows => {
            let script = format!(
                "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                ps_quote(archive),
                ps_quote(stage)
            );
            run(
                Command::new("powershell.exe").args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    &script,
                ]),
                "解压 Windows 更新包",
            )
        }
        Platform::Linux => run(
            Command::new("tar")
                .arg("-xzf")
                .arg(archive)
                .arg("-C")
                .arg(stage),
            "解压 Linux 更新包",
        ),
    }
}

fn run(command: &mut Command, action: &str) -> Result<()> {
    let output = command.output().with_context(|| format!("{action}失败"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{action}失败：{}", stderr.trim());
    }
    Ok(())
}

fn ps_quote(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

fn find_app(dir: &Path) -> Option<PathBuf> {
    let direct = dir.join("LanPaster.app");
    if direct.is_dir() {
        return Some(direct);
    }
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.extension().and_then(|ext| ext.to_str()) == Some("app") {
                return Some(path);
            }
            if let Some(found) = find_app(&path) {
                return Some(found);
            }
        }
    }
    None
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_file() && path.file_name().and_then(|file| file.to_str()) == Some(name) {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        }
    }
    None
}

/// 在下载前检查当前安装位置是否允许自更新，避免下载完才发现没有权限。
pub fn preflight_install() -> Result<()> {
    let exe = env::current_exe().context("获取当前可执行文件路径")?;
    preflight_install_at(&exe)
}

fn preflight_install_at(exe: &Path) -> Result<()> {
    if is_dev_build_path(exe) {
        bail!("当前是开发构建，已拒绝自动覆盖；请使用打包后的安装包测试更新");
    }
    match platform() {
        Platform::Macos => {
            let app = app_bundle(exe)
                .context("当前程序不在 .app 内，无法自动更新；请从 GitHub Releases 手动下载")?;
            let app_text = app.to_string_lossy();
            if app_text.contains("/AppTranslocation/") || app_text.starts_with("/Volumes/") {
                bail!("当前 App 正在从临时位置运行，请先拖到“应用程序”后再更新");
            }
            let parent = app.parent().context("定位 App 所在目录")?;
            ensure_writable(parent)
        }
        Platform::Windows => {
            let exe_name = exe
                .file_name()
                .and_then(|name| name.to_str())
                .context("当前可执行文件名无效")?;
            if exe_name != "lanpaster.exe" && exe_name != "lanpaster-console.exe" {
                bail!("无法识别的 Windows 安装位置：{}", exe.display());
            }
            let dest = exe.parent().context("定位 Windows 安装目录")?;
            ensure_writable(dest).with_context(|| {
                format!(
                    "没有写入权限，无法自动更新 {}。请将 LanPaster 解压到用户可写目录，或从 GitHub Releases 手动下载",
                    exe.display()
                )
            })
        }
        Platform::Linux => {
            if exe.starts_with("/usr/bin") || exe.starts_with("/usr/sbin") {
                bail!("当前版本由系统包管理器安装，请使用 apt 升级，或在 GitHub Releases 手动下载");
            }
            let parent = exe.parent().context("定位 Linux 安装目录")?;
            ensure_writable(parent).with_context(|| {
                format!(
                    "没有写入权限，无法自动更新 {}。通过 deb 安装的版本请使用 apt 升级，或从 GitHub Releases 手动下载",
                    exe.display()
                )
            })
        }
    }
}

/// 启动更新脚本。脚本会等待当前进程退出后再替换文件，然后重新启动应用。
pub fn spawn_installer(staged: &StagedUpdate) -> Result<()> {
    let exe = env::current_exe().context("获取当前可执行文件路径")?;
    preflight_install_at(&exe)?;
    match platform() {
        Platform::Macos => spawn_macos_installer(staged, &exe),
        Platform::Windows => spawn_windows_installer(staged, &exe),
        Platform::Linux => spawn_linux_installer(staged, &exe),
    }
}

fn spawn_macos_installer(staged: &StagedUpdate, exe: &Path) -> Result<()> {
    let app = app_bundle(exe)
        .context("当前程序不在 .app 内，无法自动更新；请从 GitHub Releases 手动下载")?;
    let app_text = app.to_string_lossy();
    if app_text.contains("/AppTranslocation/") || app_text.starts_with("/Volumes/") {
        bail!("当前 App 正在从临时位置运行，请先拖到“应用程序”后再更新");
    }
    if !staged.payload.join("Contents/MacOS/lanpaster").is_file() {
        bail!("更新包中的 LanPaster.app 结构不完整");
    }
    let parent = app.parent().context("定位 App 所在目录")?;
    ensure_writable(parent)?;
    let dir = update_dir()?;
    let script = dir.join("apply-macos.sh");
    fs::write(&script, MACOS_HELPER).context("写入 macOS 更新脚本")?;
    make_executable(&script)?;
    let log = dir.join("update.log");
    Command::new("/bin/sh")
        .arg(&script)
        .arg(process::id().to_string())
        .arg(&staged.payload)
        .arg(&app)
        .arg(&log)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("启动 macOS 更新脚本")?;
    info!(
        "macOS update {} will replace {} after exit",
        staged.version,
        app.display()
    );
    Ok(())
}

fn spawn_windows_installer(staged: &StagedUpdate, exe: &Path) -> Result<()> {
    let exe_name = exe
        .file_name()
        .and_then(|name| name.to_str())
        .context("当前可执行文件名无效")?;
    if exe_name != "lanpaster.exe" && exe_name != "lanpaster-console.exe" {
        bail!("无法识别的 Windows 安装位置：{}", exe.display());
    }
    let dest = exe.parent().context("定位 Windows 安装目录")?;
    ensure_writable(dest).with_context(|| {
        format!(
            "没有写入权限，无法自动更新 {}。请将 LanPaster 解压到用户可写目录，或从 GitHub Releases 手动下载",
            exe.display()
        )
    })?;
    let source = find_file(&staged.payload, "lanpaster.exe")
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .context("更新包中没有找到 lanpaster.exe")?;
    let dir = update_dir()?;
    let script = dir.join("apply-windows.ps1");
    fs::write(&script, WINDOWS_HELPER).context("写入 Windows 更新脚本")?;
    let log = dir.join("update.log");
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-File",
        ])
        .arg(&script)
        .arg("-TargetPid")
        .arg(process::id().to_string())
        .arg("-SourceDir")
        .arg(&source)
        .arg("-DestDir")
        .arg(dest)
        .arg("-ExeName")
        .arg(exe_name)
        .arg("-LogPath")
        .arg(&log)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.spawn().context("启动 Windows 更新脚本")?;
    info!(
        "Windows update {} will replace {} after exit",
        staged.version,
        exe.display()
    );
    Ok(())
}

fn spawn_linux_installer(staged: &StagedUpdate, exe: &Path) -> Result<()> {
    if !staged.payload.is_file() {
        bail!("更新包中的 lanpaster 可执行文件不存在");
    }
    if exe.starts_with("/usr/bin") || exe.starts_with("/usr/sbin") {
        bail!("当前版本由系统包管理器安装，请使用 apt 升级，或在 GitHub Releases 手动下载");
    }
    let parent = exe.parent().context("定位 Linux 安装目录")?;
    ensure_writable(parent).with_context(|| {
        format!(
            "没有写入权限，无法自动更新 {}。通过 deb 安装的版本请使用 apt 升级，或从 GitHub Releases 手动下载",
            exe.display()
        )
    })?;
    let dir = update_dir()?;
    let script = dir.join("apply-linux.sh");
    fs::write(&script, LINUX_HELPER).context("写入 Linux 更新脚本")?;
    make_executable(&script)?;
    let log = dir.join("update.log");
    Command::new("/bin/sh")
        .arg(&script)
        .arg(process::id().to_string())
        .arg(&staged.payload)
        .arg(exe)
        .arg(&log)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("启动 Linux 更新脚本")?;
    info!(
        "Linux update {} will replace {} after exit",
        staged.version,
        exe.display()
    );
    Ok(())
}

fn ensure_writable(dir: &Path) -> Result<()> {
    let probe = dir.join(format!(".lanpaster-write-test-{}", process::id()));
    match File::create(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            Ok(())
        }
        Err(err) => bail!("目录 {} 不可写：{err}", dir.display()),
    }
}

fn app_bundle(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("app"))
        .map(Path::to_path_buf)
}

fn is_dev_build_path(exe: &Path) -> bool {
    let mut after_target = false;
    for component in exe.components() {
        let Some(name) = component.as_os_str().to_str() else {
            continue;
        };
        if after_target && (name == "debug" || name == "release") {
            return true;
        }
        after_target = name == "target";
    }
    false
}

fn update_dir() -> Result<PathBuf> {
    let dir = config::cache_dir().join("updates");
    fs::create_dir_all(&dir).with_context(|| format!("创建更新缓存目录 {}", dir.display()))?;
    Ok(dir)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

const MACOS_HELPER: &str = r#"#!/bin/sh
# args: pid source_app target_app log
pid="$1"
src="$2"
dst="$3"
log="$4"
exec >>"$log" 2>&1
echo "[$(date '+%Y-%m-%d %H:%M:%S')] wait for pid $pid"
reopen() {
  if [ -n "${LANPASTER_CONFIG:-}" ]; then
    /usr/bin/open --env "LANPASTER_CONFIG=$LANPASTER_CONFIG" "$dst"
  else
    /usr/bin/open "$dst"
  fi
}
while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
sleep 0.5
tmp="${dst}.new.$$"
old="${dst}.old.$$"
rm -rf "$tmp" "$old"
if ! /usr/bin/ditto "$src" "$tmp"; then
  echo "copy failed"
  rm -rf "$tmp"
  reopen
  exit 1
fi
if ! /usr/bin/codesign --force --deep --sign - "$tmp"; then
  echo "codesign failed"
  rm -rf "$tmp"
  reopen
  exit 1
fi
/usr/bin/xattr -dr com.apple.quarantine "$tmp" >/dev/null 2>&1
if ! /usr/bin/codesign --verify --deep --strict "$tmp"; then
  echo "verify failed"
  rm -rf "$tmp"
  reopen
  exit 1
fi
if ! mv "$dst" "$old"; then
  echo "backup failed"
  rm -rf "$tmp"
  reopen
  exit 1
fi
if ! mv "$tmp" "$dst"; then
  echo "replace failed"
  rm -rf "$tmp"
  mv "$old" "$dst"
  reopen
  exit 1
fi
rm -rf "$old"
echo "installed; restarting"
reopen
rm -f "$0"
"#;

const LINUX_HELPER: &str = r#"#!/bin/sh
# args: pid source_bin target_bin log
pid="$1"
src="$2"
dst="$3"
log="$4"
exec >>"$log" 2>&1
echo "[$(date '+%Y-%m-%d %H:%M:%S')] wait for pid $pid"
while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
sleep 0.5
tmp="${dst}.new.$$"
if cp "$src" "$tmp" && chmod 755 "$tmp" && mv "$tmp" "$dst"; then
  echo "installed; restarting"
  nohup "$dst" >/dev/null 2>&1 &
else
  echo "replace failed"
  rm -f "$tmp"
  nohup "$dst" >/dev/null 2>&1 &
  exit 1
fi
rm -f "$0"
"#;

const WINDOWS_HELPER: &str = r#"param(
    [int]$TargetPid,
    [string]$SourceDir,
    [string]$DestDir,
    [string]$ExeName,
    [string]$LogPath
)
$ErrorActionPreference = 'Stop'
function Write-Log([string]$Message) {
    $line = (Get-Date).ToString('s') + ' ' + $Message
    Add-Content -LiteralPath $LogPath -Value $line
}
Write-Log "wait for pid $TargetPid"
while (Get-Process -Id $TargetPid -ErrorAction SilentlyContinue) {
    Start-Sleep -Milliseconds 200
}
Start-Sleep -Milliseconds 500
$names = @('lanpaster.exe', 'lanpaster-console.exe')
$backups = @{}
try {
    foreach ($name in $names) {
        $target = Join-Path $DestDir $name
        $source = Join-Path $SourceDir $name
        if ((Test-Path -LiteralPath $source) -and (Test-Path -LiteralPath $target)) {
            $backup = $target + '.old'
            if (Test-Path -LiteralPath $backup) {
                Remove-Item -LiteralPath $backup -Force
            }
            Move-Item -LiteralPath $target -Destination $backup -Force
            $backups[$name] = $backup
        }
    }
    foreach ($name in $names) {
        $source = Join-Path $SourceDir $name
        if (Test-Path -LiteralPath $source) {
            Copy-Item -LiteralPath $source -Destination (Join-Path $DestDir $name) -Force
        }
    }
    Write-Log "installed; restarting $ExeName"
    Start-Process -FilePath (Join-Path $DestDir $ExeName) -WorkingDirectory $DestDir
} catch {
    Write-Log ("update failed: " + $_.Exception.Message)
    foreach ($name in $backups.Keys) {
        $target = Join-Path $DestDir $name
        Remove-Item -LiteralPath $target -Force -ErrorAction SilentlyContinue
        Move-Item -LiteralPath $backups[$name] -Destination $target -Force -ErrorAction SilentlyContinue
    }
    Start-Process -FilePath (Join-Path $DestDir $ExeName) -WorkingDirectory $DestDir
    exit 1
} finally {
    foreach ($backup in $backups.Values) {
        Remove-Item -LiteralPath $backup -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_tags() {
        assert_eq!(parse_version_tag("v0.8.0").unwrap(), Version::new(0, 8, 0));
        assert_eq!(
            parse_version_tag("1.2.3-beta.1").unwrap(),
            Version::parse("1.2.3-beta.1").unwrap()
        );
        assert!(parse_version_tag("release").is_err());
    }

    #[test]
    fn selects_platform_assets() {
        assert_eq!(
            asset_rank("LanPaster-0.8.0-macos-universal.zip", Platform::Macos),
            Some(0)
        );
        assert_eq!(
            asset_rank("LanPaster-0.8.0-windows-x86_64.zip", Platform::Windows),
            Some(0)
        );
        assert_eq!(
            asset_rank("LanPaster-0.8.0-linux-x86_64.tar.gz", Platform::Linux),
            Some(0)
        );
        assert_eq!(
            asset_rank("lanpaster-linux-amd64", Platform::Linux),
            Some(1)
        );
        assert_eq!(
            asset_rank("LanPaster-0.8.0-macos-universal.dmg", Platform::Macos),
            None
        );
        assert_eq!(
            asset_rank("LanPaster-0.8.0-macos-universal.zip", Platform::Windows),
            None
        );
        assert_eq!(asset_rank("../evil.zip", Platform::Macos), None);
    }

    #[test]
    fn finds_checksum() {
        let sums = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  LanPaster-0.8.0-macos-universal.zip\n";
        assert_eq!(
            checksum_for(sums, "LanPaster-0.8.0-macos-universal.zip").unwrap(),
            "a".repeat(64)
        );
        assert!(checksum_for(sums, "LanPaster-0.8.0-windows-x86_64.zip").is_err());
    }

    #[test]
    fn ignores_same_or_older_release() {
        let body = r#"{
            "tag_name": "v0.7.12",
            "html_url": "https://example.invalid",
            "assets": []
        }"#;
        assert!(parse_release(body, &Version::new(0, 7, 12))
            .unwrap()
            .is_none());
    }

    #[test]
    fn parses_newer_release() {
        let body = r#"{
            "tag_name": "v0.8.0",
            "html_url": "https://example.invalid/release",
            "body": "notes",
            "assets": [
                {"name": "LanPaster-0.8.0-macos-universal.zip", "browser_download_url": "https://example.invalid/app.zip", "size": 123},
                {"name": "LanPaster-0.8.0-windows-x86_64.zip", "browser_download_url": "https://example.invalid/app-win.zip", "size": 123},
                {"name": "LanPaster-0.8.0-linux-x86_64.tar.gz", "browser_download_url": "https://example.invalid/app-linux.tar.gz", "size": 123},
                {"name": "SHA256SUMS.txt", "browser_download_url": "https://example.invalid/SHA256SUMS.txt", "size": 123}
            ]
        }"#;
        let info = parse_release(body, &Version::new(0, 7, 12))
            .unwrap()
            .unwrap();
        assert_eq!(info.version, Version::new(0, 8, 0));
        assert!(info.asset_name.starts_with("LanPaster-0.8.0-"));
    }

    #[test]
    fn dev_build_paths_are_rejected() {
        assert!(is_dev_build_path(Path::new(
            "/Users/me/LanPaster/target/release/lanpaster"
        )));
        assert!(!is_dev_build_path(Path::new(
            "/Applications/LanPaster.app/Contents/MacOS/lanpaster"
        )));
    }
}
