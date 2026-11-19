# LanPaster

局域网剪贴板同步工具的可运行版本。

当前版本包含同步内核和桌面入口：**文本 / 图片 / 文件** 三类剪贴内容、设备身份 + 首次连接批准、mDNS 发现与手动连接、请求连接、手动 IP 直连、大小阈值、文件临时落地、内存剪贴历史、原生系统托盘、单窗口设置中心（Cupertino 风格，左侧连接 / 本地）、提示音、开机自启、单实例和配置热加载。

## 已实现

- 文本剪贴同步（UTF-8，按大小上限过滤）
- 图片剪贴同步：优先保留剪贴板提供的原始编码格式（PNG / JPEG / WebP / GIF / HEIC / TIFF），拿不到原格式时转 PNG；接收端优先按原格式写回
- 文件剪贴同步：
  - 支持 macOS file-url、Windows CF_HDROP、Linux text/uri-list（通过 `clipboard-rs`）
  - 清单协商 + 分块流式传输 + SHA-256 校验
  - 接收端先写临时文件，全部成功后再写入文件剪贴板
  - 文件名清洗、防路径穿越、重名加后缀、不覆盖、不自动执行
  - 单文件 / 单次总量 / 文件数量上限
- 连接与安全：
  - TCP P2P，Noise `XX` 握手（`snow`），持久 X25519 设备身份
  - 首次连接由一方发起请求，另一方在托盘“连接请求”里点击“同意”或“拒绝”
  - 批准后写入本地 `trusted.toml`，后续连接不再重复确认（mDNS 发现不会自动拨号）
  - 每次连接双方在 Hello 中声明是否仍信任对方；任一方移除后，另一方立即同步删除并断开
  - 被移除设备记入 `revoked` 列表，不会自动重新配对，需在设置里恢复
  - 每连接心跳、断线重连
  - 双向拨号去重（按设备 ID 选 canonical 连接），避免连接风暴
  - mDNS 发现（`mdns-sd`），TXT 只放设备 ID / 名称 / 协议版本；只发现不自动连接，托盘里手动扫描并点击连接
  - 手动 `peers = ["host:port"]` 兜底（配置项仍会自动拨号）
- 本地配置：`~/Library/Application Support/LanPaster/config.toml`（macOS）、`~/.config/LanPaster/config.toml`（Linux）、`%APPDATA%\LanPaster\config.toml`（Windows）
- 首次运行生成设备 ID、持久 X25519 身份密钥；配置和信任文件权限 0600
- 循环抑制：本地事件指纹 + 远端写入后的下一次轮询抑制 + 事件 ID 去重
- 原生系统托盘（`tray-icon` + `muda` + `winit`）：
  - 菜单栏图标：实心 LP 字母 + 圆角方框，macOS template 图标，自动适配深浅色；离线 / 在线 / 收到剪贴三种状态
  - 启用 / 暂停剪贴同步（勾选菜单项）
  - 连接请求子菜单：同意 / 拒绝
  - 在线设备子菜单，自动刷新
  - 打开设置中心
  - 推送当前剪贴板
  - 退出程序
  - macOS 使用 Accessory 激活策略，不显示 Dock 图标
- 配置面板是单窗口设置中心：左侧“连接 / 本地”导航，托盘菜单“连接设置 / 本地设置”直接定位到对应页；使用 Slint Cupertino 风格，浅灰背景 + 白色分组卡片 + 细分隔线 + 系统风开关；关闭即退出，不常驻
- 无界面模式：`lanpaster run --headless`，适合服务器 / 调试 / 无桌面环境
- 提示音：收到远端剪贴后播放短提示音；托盘菜单“测试提示音”，配置面板可关闭
- 开机自启：macOS LaunchAgent、Windows 注册表 Run、Linux XDG autostart；托盘菜单可勾选，也可用 CLI
- 单实例：按配置文件路径加文件锁；同一配置只允许一个守护进程
- 配置热加载：阈值、文件同步、提示音、轮询间隔、历史保留时长 / 条数修改后自动生效；端口、mDNS、手动节点、设备身份仍需重启
- 剪贴板监听优化：优先使用 `clipboard-rs` 平台 watcher（macOS 实测生效），失败时退回轮询；watcher 有效时保留 10 秒兜底轮询
- 接收文件缓存：按 `cache_max_mb` 自动删除旧事件目录，保留最新一次文件剪贴
- 内存剪贴历史：最近 `history_minutes` 分钟内最多 `history_limit` 条，支持文本 / 图片 / 文件，托盘菜单可点击恢复到剪贴板；不落盘

## 尚未实现

- 剪贴历史持久化（当前历史仅保存在内存，退出即清空）
- Linux GNOME Wayland 的原生后台剪贴板监听（Mutter 不实现 data-control；目前只能依赖 XWayland 或桌面扩展）
- 目录、符号链接、断点续传（一期仅常规文件）
- `.app` / Windows exe / Linux AppImage 打包
- Windows / Linux 实机验证

## 构建

需要 Rust stable（本项目在 macOS arm64 上验证；Windows / Linux 代码路径已写但尚未实测）。

```bash
cargo build --release
./target/release/lanpaster init
```

`init` 会创建配置并打印：

```
config: /Users/xxx/Library/Application Support/LanPaster/config.toml
device: my-mac (xxxxxxxxxxxxxxxx)
fingerprint: A1B2C3D4E5F60708
```

每台设备的 `device_id` 和身份密钥必须不同（留空会自动生成），`listen_port` 不能冲突。不要把 config.toml 复制到另一台设备，否则会复制同一份设备身份。

## 运行

默认启动托盘（不显示 Dock 图标）：

```bash
./target/release/lanpaster run
```

无桌面环境 / 调试时使用无界面模式：

```bash
./target/release/lanpaster run --headless
```

单独打开设置中心（托盘菜单里“连接设置”和“本地设置”会直接定位到对应页）：

```bash
# 打开连接页：设备名、设备指纹、端口、mDNS、手动节点
./target/release/lanpaster ui connection

# 打开本地页：轮询、文本/文件大小、文件同步、提示音
./target/release/lanpaster ui local
```

不传参数时默认打开连接页。

首次连接流程：

1. 两台设备都启动 LanPaster，保持同一局域网；
2. 托盘菜单点击「扫描局域网设备」，在「发现的设备」里点击目标设备发起连接；
3. 接收方托盘菜单出现“连接请求”，展开后点击“同意”；
4. 批准后保存到 `config.trusted.toml`（与配置文件同目录），之后从「发现的设备」连接时不再重复确认；
5. 如果对端重装或身份密钥变化，会重新请求确认。

提示音测试与开机自启：

```bash
./target/release/lanpaster beep
./target/release/lanpaster autostart enable
./target/release/lanpaster autostart disable
./target/release/lanpaster autostart status
```

开机自启使用默认配置路径；如果使用 `LANPASTER_CONFIG` 自定义配置，建议手动调整启动项或在默认路径放配置。

默认开启 mDNS 发现，但**不会自动建立连接**：托盘菜单「扫描局域网设备」刷新局域网列表，「发现的设备」里点击设备才会发起连接；协议版本不一致的设备会标注「版本不兼容」。也可以在配置里关闭 mDNS，使用手动直连：

```toml
mdns_enabled = false
peers = ["192.168.1.20:39393", "192.168.1.21:39393"]
```

启动日志会显示已连接设备。按 `Ctrl-C` 退出。

## 信任设备管理

打开设置中心的「连接」页，底部「已信任设备」区块会列出所有已批准的设备（名称、指纹、设备 ID）：

- 点某台设备右侧的「移除」即可取消信任该设备，正在使用的连接会立即断开
- 点「清空全部」删除所有信任设备
- 移除会通过已建立的加密连接通知对端，对端同步删除对应记录；如果对端当时离线，会在下次重连握手时发现双方信任状态不一致并自动删除

设备信息保存在配置同目录的 `config.trusted.toml`（默认 `~/.config/LanPaster/config.trusted.toml`）。被移除的设备会进入「已移除设备」列表，双方都不会再自动发起配对，避免刚删掉又被 mDNS 重新配回来。需要重新连接时，在双方设置里点「恢复」（或「全部恢复」），等下一次连接重新确认配对即可。

使用 `LANPASTER_CONFIG=/path/xxx.toml` 自定义配置时，信任文件路径为 `/path/xxx.trusted.toml`。

## macOS 打包

```bash
./scripts/package-macos.sh
```

默认构建 Universal（arm64 + x86_64）App，产物在 `dist/`：

- `dist/LanPaster.app`
- `dist/LanPaster-<version>-macos-universal.dmg`
- `dist/LanPaster-<version>-macos-universal.zip`

只构建当前 arm64 可以加：

```bash
ONLY_ARM64=1 ./scripts/package-macos.sh
```

App 使用 ad-hoc 签名，没有 Apple 开发者签名 / 公证。自己本机运行通常没问题；如果从网络下载后被 Gatekeeper 拦截，右键“打开”，或执行：

```bash
xattr -dr com.apple.quarantine /Applications/LanPaster.app
```

## 测试 / 手动推送

这些命令用于调试，也可以当作“手动推送剪贴板”的临时替代：

```bash
# 向某台设备发送文本
./target/release/lanpaster send 192.168.1.20:39393 "hello"

# 发送单个文件
./target/release/lanpaster send-file 192.168.1.20:39393 ./a.pdf

# 发送多个文件
./target/release/lanpaster send-files 192.168.1.20:39393 ./a.pdf ./b.png

# 发送图片（保留文件内编码格式）
./target/release/lanpaster send-image 192.168.1.20:39393 ./photo.jpg
```

发送命令使用当前配置里的设备身份；如果对方尚未信任，需要对方在托盘确认。

## 配置项

| 键 | 默认 | 说明 |
|---|---:|---|
| `device_name` | 主机名 | 对端显示名称 |
| `device_id` | 自动生成 | 设备唯一 ID，不要与其他设备重复 |
| `private_key` / `public_key` | 自动生成 | X25519 设备身份，不要复制或分享 |
| `listen_port` | 39393 | TCP 监听端口 |
| `poll_interval_ms` | 500 | 剪贴板轮询间隔 |
| `max_text_kb` | 1024 | 单条文本上限 |
| `max_file_mb` | 100 | 单文件 / 图片上限（按编码后字节计） |
| `max_batch_mb` | 200 | 单次剪贴的总量上限 |
| `max_files` | 50 | 单次文件数量上限 |
| `file_sync` | true | 文件同步开关 |
| `sound_enabled` | true | 收到远端剪贴时是否播放提示音 |
| `cache_max_mb` | 1024 | 接收临时文件缓存上限，超出清理旧事件目录 |
| `history_minutes` | 10 | 内存剪贴历史保留时长（分钟），0 表示关闭 |
| `history_limit` | 20 | 内存剪贴历史最多条目数，0 表示关闭 |
| `mdns_enabled` | true | mDNS 自动发现开关 |
| `peers` | `[]` | 手动直连地址列表 |
| `log_level` | info | `RUST_LOG` 可覆盖，如 `RUST_LOG=debug` |

**限流规则：** 文本、单文件（图片也按一个文件项处理）、总量任一超限，整次剪贴事件不同步，不做部分同步。图片只看编码后字节大小，不限制像素；文件在读取前先用元数据判断大小。

## 接收文件

文件落在系统缓存目录：

- macOS: `~/Library/Caches/LanPaster/incoming/<事件目录>/`
- Linux: `~/.cache/LanPaster/incoming/...`
- Windows: `%LOCALAPPDATA%\LanPaster\cache\incoming\...`

接收成功后会把本地临时文件列表写入剪贴板，可以直接在 Finder / Explorer / 文件管理器中粘贴。每次成功接收文件后会检查 `cache_max_mb`，按目录时间删除旧事件目录，最新一次文件剪贴会保留。程序启动时也会清空旧接收目录。

## 剪贴历史

历史只保存在内存里，进程退出即清空，不会写入磁盘：

- `history_minutes`：只保留最近 N 分钟内的内容，默认 10 分钟
- `history_limit`：最多保留 M 条，默认 20 条
- 任一为 0 即关闭历史
- 托盘菜单「剪贴历史」按时间倒序列出，点击即可恢复到系统剪贴板
- 托盘菜单「清空剪贴历史」可立即清空全部历史
- 文本 / 图片会完整保存在内存中，文件只保存路径；源文件或接收缓存被清理后，对应历史项会标记为「文件已失效」
- 相同内容再次复制会移到列表最前面，不会重复占位

注意：图片按 `max_file_mb` 限制完整保存在内存中，调大图片上限或历史条目数时要留意内存占用。历史里是剪贴板明文内容，复制过密码等敏感信息后可用「清空剪贴历史」立即清除。

## 安全说明

- 设备身份私钥只本地保存，不通过 mDNS 或网络明文传输。
- 首次连接必须由对方在托盘确认；批准记录保存在 `config.trusted.toml`。
- 配置文件和接收目录权限分别设为 0600 / 0700（Unix）。
- 接收端不自动打开文件，不保留可执行 / setuid 权限。
- 自用工具，未做系统级密钥链存储、证书固定、审计日志等。

## 代码结构

- `src/main.rs`：CLI、启动流程
- `src/config.rs`：配置、默认值、设备身份 / ID
- `src/pairing.rs`：首次连接请求、信任设备持久化
- `src/clipboard.rs`：剪贴板轮询、图片格式选择、文件元数据、远端写入
- `src/protocol.rs`：消息定义、bincode 编解码、chunk 大小
- `src/crypto.rs`：Noise 握手与加密帧
- `src/net.rs`：TCP 监听、拨号、连接管理、文件/图片/文本传输、限额校验
- `src/history.rs`：内存剪贴历史（按时间和条数淘汰）
- `src/mdns.rs`：mDNS 注册与发现
- `src/tray.rs`：原生托盘、菜单事件、winit 事件循环
- `src/ui.rs`：Slint 配置面板（独立进程）
- `src/sound.rs`：提示音
- `src/autostart.rs`：开机自启（macOS / Windows / Linux）
- `src/instance.rs`：单实例文件锁
- `src/cache.rs`：接收文件缓存清理
- `ui/app.slint`：配置窗口布局
- `assets/tray-*.png`：托盘图标
- `src/client.rs`：`send` / `send-file` / `send-files` / `send-image` 调试客户端
- `scripts/package-macos.sh`：macOS Universal App / DMG / ZIP 打包
- `assets/AppIcon.icns`：App 图标

## 已验证

在 macOS arm64 上实测通过：

- 文本、JPEG 图片、单文件、多文件同步
- mDNS 能发现局域网设备；手动连接后稳定传输（发现本身不再自动拨号）
- 两个守护进程通过手动 `peers` 直连
- 超过 `max_file_mb` 的文件被拒绝，客户端收到 Reject
- 首次连接请求、托盘同意后写入 trusted.toml，后续自动连接
- 一端移除信任后连接立即断开，对端同步删除信任并进入「已移除设备」列表
- 原生托盘启动、设备列表更新、菜单事件路径
- 配置面板独立进程启动
- 提示音命令、开机自启写入/移除
- 同一配置第二个实例被拒绝
- 修改 `max_text_kb` 后无需重启立即生效
- 内存剪贴历史的 N/M 淘汰、去重、文件失效过滤（逻辑单测覆盖）
- `clipboard-rs` 平台 watcher 在 macOS 上生效
- 两次 700KB 文件接收后只保留最新事件目录
- Universal `.app`（arm64 + x86_64）构建、ad-hoc 签名、DMG 挂载验证通过
- 从 `.app` 启动托盘进程和配置 UI 进程均正常

资源占用（release，macOS arm64 实测）：

- 无界面模式：约 18–20 MB，空闲 CPU 接近 0
- 带原生托盘：约 80 MB（AppKit / NSStatusItem 基线）
- 配置面板进程：约 125 MB，仅在打开期间存在

## 下一步

1. Windows / Linux 实机验证与打包
2. macOS 公证 / Developer ID 签名（可选）
3. 配置热加载扩展到端口 / mDNS / 手动节点
4. 持久化剪贴历史、设备管理
5. 目录同步 / 断点续传
