# LanPaster

[English](README.md) · **简体中文** · [日本語](README.ja.md)

[![最新版本](https://img.shields.io/github/v/release/Maythelos/LanPaster?label=release)](https://github.com/Maythelos/LanPaster/releases/latest)
[![许可证：GPL-3.0-only](https://img.shields.io/badge/license-GPL--3.0--only-blue)](LICENSE)
![平台](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey)

LanPaster 是一个常驻系统托盘的局域网剪贴板同步工具，支持 **文本、图片和文件**。
它让同一局域网内的设备通过加密的 TCP 连接直接互传剪贴内容，不经过云端，也不需要账号。

> **项目状态：**目前只有 macOS arm64 在实机上完整验证过。Windows / Linux 的代码
> 路径已经实现并打包，但仍需要更多实机测试。欢迎提交问题和测试结果。

## 下载

请从 **[GitHub Releases](https://github.com/Maythelos/LanPaster/releases/latest)** 下载最新版本。

| 平台 | 架构 | 安装包 |
| --- | --- | --- |
| macOS 13+ | Universal（arm64 + x86_64） | `.dmg`（推荐）或 `.zip` |
| Windows 10/11 | x86_64 | `.zip`：`lanpaster.exe`（GUI）+ `lanpaster-console.exe` |
| Linux | x86_64 | `.deb`（Ubuntu/Debian）或 `.tar.gz` |

Release 页面同时提供 `SHA256SUMS.txt` 用于校验。

<details>
<summary>安装说明</summary>

**macOS** — 打开 DMG，把 LanPaster 拖进“应用程序”后启动。App 使用 ad-hoc
签名，首次打开可能被 Gatekeeper 拦截；右键 App 选择“打开”，或执行：

```bash
xattr -dr com.apple.quarantine /Applications/LanPaster.app
```

**Windows** — 解压后运行 `lanpaster.exe`。程序没有代码签名，SmartScreen 可能
提示；选择“更多信息”→“仍要运行”。`lanpaster-console.exe` 是带控制台的同一
程序，适合执行 `init`、`version` 或查看日志。

**Linux** — Ubuntu/Debian 可以执行：

```bash
sudo apt install ./LanPaster-<version>-linux-amd64.deb
```

也可以解压 tar.gz 后运行 `./usr/bin/lanpaster`，并按 Release 页面的说明安装
运行依赖。Linux 二进制在 Ubuntu 22.04（glibc 2.35+）上构建。

</details>

## 主要特性

- **三类剪贴内容** — UTF-8 文本、图片、文件列表。
- **图片格式保留** — 优先保留剪贴板提供的原始编码（PNG、JPEG、WebP、GIF、
  HEIC、TIFF），拿不到原格式时转成 PNG。
- **流式文件传输** — 清单协商、分块传输、SHA-256 校验；接收端先写临时缓存，
  全部成功后才写入系统剪贴板。
- **显式信任** — Noise `XX` 握手 + 持久化 X25519 设备身份；首次连接必须在
  托盘中手动同意。信任设备保存在本地，被移除的设备会被记住。
- **只发现，不自动连接** — mDNS 只负责发现设备；在托盘里点击设备才会发起连接。
  也支持手动配置 `host:port` 节点。
- **原生托盘体验** — 同步开关、发现的设备、在线设备、连接请求、剪贴历史、
  设置、开机自启、测试提示音、退出。macOS 使用模板菜单栏图标，不显示 Dock 图标。
- **轻量设置中心** — 单个 Slint 窗口，包含“连接 / 本地”两页；设置窗口是独立
  进程，关闭即退出，再次点击托盘图标会复用已打开的窗口。
- **实用默认值** — 大小 / 数量限制、文件缓存清理、内存剪贴历史、单实例锁，
  以及大部分配置的热加载。
- **手动 GitHub 自更新** — 只有你从托盘菜单点击「检查更新」时才联网查询最新
  Release；下载后用 `SHA256SUMS.txt` 校验，再等你确认才替换应用或二进制。

## 工作原理

1. **发现** — 通过 mDNS 广播设备 ID、名称和协议版本。发现本身不会自动连接。
2. **配对** — 双方用持久化 X25519 身份执行 Noise `XX` 握手；接收方在托盘
   点击“同意”后，指纹写入 `<配置文件>.trusted.toml`。
3. **同步** — 本地剪贴板发生变化后，内容被编码成加密消息直接发给已连接设备。
   文件先协商清单，再分块传输并做 SHA-256 校验。
4. **接收** — 文件先落到按事件划分的缓存目录；全部写成功后，才把文件列表写入
   本地剪贴板。接收端不会保留可执行 / setuid 权限，也不会自动打开文件。

## 快速开始

1. 在同一局域网的两台设备上启动 LanPaster。
2. 托盘菜单 →「扫描局域网设备」→ 点击对方设备。
3. 在接收方托盘菜单 →「连接请求」→「同意」。
4. 批准后再次连接不再询问，剪贴板变化会自动同步。

mDNS 不可用时，可以手动配置节点：

```toml
mdns_enabled = false
peers = ["192.168.1.20:39393", "192.168.1.21:39393"]
```

服务器或调试场景可以不开托盘：

```bash
lanpaster run --headless
# 或者：LANPASTER_HEADLESS=1 lanpaster run
```

也可以单独打开设置窗口：

```bash
lanpaster ui connection   # 设备名、指纹、端口、mDNS、手动节点
lanpaster ui local        # 轮询、限制、文件同步、提示音、历史
```

## 配置

默认配置文件位置：

| 平台 | 路径 |
| --- | --- |
| macOS | `~/Library/Application Support/LanPaster/config.toml` |
| Linux | `~/.config/LanPaster/config.toml` |
| Windows | `%APPDATA%\LanPaster\config.toml` |

信任设备文件与配置文件同目录，名字是 `<配置文件名>.trusted.toml`（默认
`config.trusted.toml`）。如果用 `LANPASTER_CONFIG=/path/foo.toml`，信任文件
就是 `/path/foo.trusted.toml`。

| 配置项 | 默认值 | 说明 |
| --- | ---: | --- |
| `device_name` | 主机名 | 对端显示的名称 |
| `device_id` | 自动生成 | 设备唯一 ID，不要在多台设备间复用 |
| `private_key` / `public_key` | 自动生成 | X25519 身份，不要复制或分享 |
| `listen_port` | `39393` | TCP 监听端口 |
| `poll_interval_ms` | `500` | 剪贴板轮询间隔（毫秒） |
| `max_text_kb` | `1024` | 单条文本上限 |
| `max_file_mb` | `100` | 单个文件 / 图片上限（按编码后字节计） |
| `max_batch_mb` | `200` | 单次剪贴事件的总量上限 |
| `max_files` | `50` | 单次文件数量上限 |
| `file_sync` | `true` | 是否同步文件 |
| `sound_enabled` | `true` | 收到远端剪贴后是否播放提示音 |
| `cache_max_mb` | `1024` | 接收文件缓存上限 |
| `history_minutes` | `10` | 内存历史保留时长（分钟），`0` 表示关闭 |
| `history_limit` | `20` | 内存历史最多条数，`0` 表示关闭 |
| `mdns_enabled` | `true` | 是否启用 mDNS 发现 |
| `peers` | `[]` | 手动节点列表，格式为 `host:port`；启动后自动拨号 |
| `log_level` | `info` | 日志级别，可被 `RUST_LOG` 覆盖 |

**限额按整次事件生效。** 文本、单文件、总量、文件数量任一超限，整次剪贴事件
都不会同步，不做部分同步。图片按一个文件项处理，只限制编码后字节数，不限制
像素。

**热加载** — 以下配置修改后大约 1 秒内生效，无需重启：

`max_text_kb`、`max_file_mb`、`max_batch_mb`、`max_files`、`file_sync`、
`sound_enabled`、`poll_interval_ms`、`history_minutes`、`history_limit`。

修改 `listen_port`、`mdns_enabled`、`peers`、`device_id` 或设备密钥后需要重启。

### 更新

更新完全手动。托盘菜单里有「检查更新」，启动时和后台都不会主动查询。点击后
LanPaster 才会查询 GitHub 最新 Release，选择当前平台的安装包，下载后按
`SHA256SUMS.txt` 校验 SHA-256。发现新版本时菜单项会变成「更新到 vX…」，在你
再次点击确认前不会下载安装，也不会重启。`lanpaster update check` 只从命令行
检查、不下载。

macOS 要求 `.app` 位于“应用程序”或其他用户可写目录；App Translocation
和只读卷会被拒绝。Linux 的 `.deb` 安装由 dpkg 管理，不会自更新；请使用
`apt` 或 Release 页面。校验和只能防止下载被截断或篡改，不能替代代码签名。

## 命令行

| 命令 | 说明 |
| --- | --- |
| `lanpaster` / `lanpaster run` | 启动托盘守护进程 |
| `lanpaster run --headless` | 无界面运行（服务器 / 调试） |
| `lanpaster init` | 创建配置并打印设备身份 / 指纹 |
| `lanpaster ui [connection\|local]` | 打开设置窗口 |
| `lanpaster beep` | 播放提示音 |
| `lanpaster autostart <enable\|disable\|status>` | 管理开机自启 |
| `lanpaster update [check]` | 查询 GitHub Releases，不下载安装 |
| `lanpaster config` | 打印当前配置 |
| `lanpaster version` | 打印版本号 |
| `lanpaster help` | 显示命令行帮助 |

环境变量：

- `LANPASTER_CONFIG=/path/to/config.toml` — 使用自定义配置路径。
- `LANPASTER_HEADLESS=1` — 等价于 `run --headless`。
- `RUST_LOG=debug` — 覆盖配置里的日志级别。

开机自启使用默认配置路径；如果使用 `LANPASTER_CONFIG` 自定义配置，请自行调整
登录启动项。

## 从源码构建

依赖：

- Rust stable **1.80+**
- macOS：Xcode Command Line Tools
- Windows：Visual Studio Build Tools（MSVC）或 GNU 工具链
- Linux：`pkg-config`、ALSA、X11/xcb、Wayland、fontconfig、freetype 开发包

```bash
cargo build --release
cargo test
./target/release/lanpaster init
```

`Cargo.toml` 里的 `version` 是版本号唯一来源，打包文件名也从这里读取。

### 打包

- **macOS** — `./scripts/package-macos.sh` 生成 Universal `.app`、DMG 和 ZIP，
  输出到 `dist/`。`ONLY_ARM64=1` 可只构建 arm64，适合快速验证。App 使用
  ad-hoc 签名。
- **Linux** — `./scripts/package-linux.sh` 在 Ubuntu 22.04 Docker 容器中构建
  `.deb` 和 `.tar.gz`；如果 `docker` 不在 `PATH`，用 `DOCKER=` 指定。
- **Windows** — Release 里的 ZIP 目前使用 LLVM-MinGW
  (`x86_64-pc-windows-gnullvm`) 交叉编译，包含 GUI 和 console 两个 exe。开发时
  也可以直接在 Windows 上执行 `cargo build --release`；原生构建默认是 console
  子系统。
- **校验和** — 当前版本的所有平台产物都放进 `dist/` 后，运行
  `./scripts/checksums.sh` 重新生成 `SHA256SUMS.txt`。自更新会拒绝没有该文件或
  缺少对应条目的 Release。

## 安全与隐私

- 剪贴板数据**只在局域网内的设备之间传输**，不经过云端、中继或账号系统。
- 传输层使用 Noise `XX` 握手加密，设备身份是持久化 X25519 密钥对。
- 首次连接必须手动同意；信任设备保存在本地 `*.trusted.toml`。移除设备会通知
  对端并记录撤销，避免被 mDNS 重新自动配对。
- 在 Unix 上，配置和信任文件以 `0600` 权限写入；接收文件目录以 `0700` 创建。
- `config.toml` 里包含私钥，`lanpaster config` 会打印完整配置；分享前务必先
  涂掉私钥。
- 接收端不会自动打开文件，也不会保留可执行 / setuid 权限。
- 自更新通过 HTTPS 下载，并用 Release 里的 `SHA256SUMS.txt` 校验。Release
  没有校验和之外的独立签名，因此 GitHub 账号被盗时仍可能发布恶意更新。
- **尚未实现：**系统钥匙串存储、证书固定、签名审计日志，或对本地账号被攻破的
  防护。请把它当作“信任局域网”的工具，而不是高强度安全通信软件。
- 剪贴历史保存在内存中且是明文；复制过密码等敏感内容后，建议使用「清空剪贴
  历史」。

如果发现安全问题，请使用 GitHub 的私密 Security Advisory，而不是公开 issue。
详见 [SECURITY.md](SECURITY.md)。

## 平台支持

| 平台 | 状态 | 说明 |
| --- | --- | --- |
| macOS 13+ arm64 | ✅ 已测试 | 主要开发平台，Apple Silicon |
| macOS x86_64 | ⚠️ 已构建 | 包含在 Universal 二进制中，运行验证较少 |
| Windows 10/11 x64 | ⚠️ 已实现 | 已打包，仍需要实机验证 |
| Linux x86_64 | ⚠️ 已实现 | 在 Ubuntu 22.04 上打包，仍需要实机验证 |

已知限制：GNOME Wayland 下 Mutter 不提供 data-control 协议，原生后台剪贴板
监听受限；可以尝试 XWayland 或桌面扩展。这是上游限制。

## 项目结构

```text
src/
  main.rs        CLI、启动流程、服务组装
  config.rs      配置、默认值、设备身份
  clipboard.rs   剪贴板监听、图片格式、文件处理
  protocol.rs    加密消息定义与分帧
  crypto.rs      Noise 握手封装
  net.rs         TCP 监听、拨号、连接、传输
  pairing.rs     首次连接审批、信任设备持久化
  mdns.rs        mDNS 注册与发现
  tray.rs        原生托盘与菜单
  ui.rs          Slint 设置窗口进程
  history.rs     内存剪贴历史
  cache.rs       接收文件缓存清理
  sound.rs       提示音
  autostart.rs   开机自启
  instance.rs    单实例锁
  updater.rs     GitHub Releases 自更新
ui/app.slint     设置窗口布局
assets/          App 与托盘图标
packaging/       各平台打包文件
scripts/         发布打包脚本
```

## 限制与路线图

- 剪贴历史只保存在内存，进程退出即清空。
- 暂不支持目录、符号链接和断点续传。
- 剪贴历史不落盘；信任设备信息保存在 TOML 文件中。
- Windows / Linux 还需要更多实机验证。
- UI 文案目前是简体中文，欢迎补充其他语言。
- Linux GNOME Wayland 的后台剪贴板监听仍受限。
- 尚未提供原生安装器 / 包管理器分发。

## 常见问题

- **发现不到设备** — 确认两台设备在同一网段、mDNS 已开启，防火墙允许 UDP
  5353 和配置的 TCP 端口。mDNS 不可用时可以配置 `peers = ["host:port"]`。
- **版本不兼容** — 协议版本不同的设备会在“发现的设备”里标注；请双方升级到
  同一版本。
- **端口被占用** — 修改 `listen_port` 后重启。
- **连接没有被批准** — 检查托盘里的「连接请求」子菜单；每个新身份都需要批准。
- **不要复制设备身份** — 不要把 `config.toml` 复制到另一台设备，否则两台设备
  会拥有同一份私钥，被对端当成同一个 peer。
- **接收文件在哪** — macOS：`~/Library/Caches/LanPaster/incoming`；Windows：
  `%LOCALAPPDATA%\LanPaster\cache\incoming`；Linux：
  `~/.cache/LanPaster/incoming`。
- **macOS Gatekeeper / Windows SmartScreen** — Release 二进制没有付费证书签名
  或公证；按上文右键“打开”或“更多信息 → 仍要运行”。

## 参与贡献

欢迎提交 issue 和 pull request。构建 / 测试命令、平台状态和提交约定见
[CONTRIBUTING.md](CONTRIBUTING.md)。

## 许可证

LanPaster 使用 **GNU General Public License v3.0 only**（`GPL-3.0-only`）发布。
详见 [LICENSE](LICENSE)。
