# LanPaster

局域网剪贴板同步工具的可运行版本。

当前版本包含同步内核和桌面入口：**文本 / 图片 / 文件** 三类剪贴内容、PSK 加密连接、mDNS 自动发现、手动 IP 直连、大小阈值、文件临时落地、原生系统托盘、按需拉起的配置面板、提示音、开机自启、单实例和配置热加载。

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
  - TCP P2P，Noise `XXpsk3` 握手（`snow`），PSK 双向认证
  - 每连接心跳、断线重连
  - 双向拨号去重（按设备 ID 选 canonical 连接），避免连接风暴
  - mDNS 自动发现（`mdns-sd`），TXT 只放设备 ID / 名称 / 协议版本，不放密钥
  - 手动 `peers = ["host:port"]` 兜底
- 本地配置：`~/Library/Application Support/LanPaster/config.toml`（macOS）、`~/.config/LanPaster/config.toml`（Linux）、`%APPDATA%\LanPaster\config.toml`（Windows）
- 首次运行生成 32 字节随机 PSK 和设备 ID；配置权限 0600
- 循环抑制：本地事件指纹 + 远端写入后的下一次轮询抑制 + 事件 ID 去重
- 原生系统托盘（`tray-icon` + `muda` + `winit`）：
  - 启用 / 暂停剪贴同步（勾选菜单项）
  - 在线设备子菜单，自动刷新
  - 打开配置面板
  - 推送当前剪贴板
  - 退出程序
  - macOS 使用 Accessory 激活策略，不显示 Dock 图标
- 配置面板：独立进程按需拉起（`lanpaster ui`），关闭即退出，不常驻；保存 TOML 配置
- 无界面模式：`lanpaster run --headless`，适合服务器 / 调试 / 无桌面环境
- 提示音：收到远端剪贴后播放短提示音；托盘菜单“测试提示音”，配置面板可关闭
- 开机自启：macOS LaunchAgent、Windows 注册表 Run、Linux XDG autostart；托盘菜单可勾选，也可用 CLI
- 单实例：按配置文件路径加文件锁；同一配置只允许一个守护进程
- 配置热加载：阈值、文件同步、提示音、轮询间隔修改后自动生效；端口、密钥、mDNS、手动节点仍需重启
- 剪贴板监听优化：优先使用 `clipboard-rs` 平台 watcher（macOS 实测生效），失败时退回轮询；watcher 有效时保留 10 秒兜底轮询
- 接收文件缓存：按 `cache_max_mb` 自动删除旧事件目录，保留最新一次文件剪贴

## 尚未实现

- 剪贴历史
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
psk:    0123...（64 位 hex）
```

把同一个 `psk` 配置到其他设备；每台设备的 `device_id` 必须不同（留空会自动生成），`listen_port` 不能冲突。

## 运行

默认启动托盘（不显示 Dock 图标）：

```bash
./target/release/lanpaster run
```

无桌面环境 / 调试时使用无界面模式：

```bash
./target/release/lanpaster run --headless
```

单独打开配置面板（托盘菜单“打开配置面板”也是启动这个子进程）：

```bash
./target/release/lanpaster ui
```

提示音测试与开机自启：

```bash
./target/release/lanpaster beep
./target/release/lanpaster autostart enable
./target/release/lanpaster autostart disable
./target/release/lanpaster autostart status
```

开机自启使用默认配置路径；如果使用 `LANPASTER_CONFIG` 自定义配置，建议手动调整启动项或在默认路径放配置。

默认开启 mDNS 自动发现。也可以在配置里关闭 mDNS，使用手动直连：

```toml
mdns_enabled = false
peers = ["192.168.1.20:39393", "192.168.1.21:39393"]
```

启动日志会显示已连接设备。按 `Ctrl-C` 退出。

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

发送命令使用当前配置里的 PSK 和设备 ID；可以复制一份配置，修改 `device_id` / `device_name` 作为测试客户端。

## 配置项

| 键 | 默认 | 说明 |
|---|---:|---|
| `device_name` | 主机名 | 对端显示名称 |
| `device_id` | 自动生成 | 设备唯一 ID，不要与其他设备重复 |
| `psk` | 自动生成 | 32 字节 hex，所有设备必须一致 |
| `listen_port` | 39393 | TCP 监听端口 |
| `poll_interval_ms` | 500 | 剪贴板轮询间隔 |
| `max_text_kb` | 1024 | 单条文本上限 |
| `max_image_mb` | 20 | 图片原始编码字节上限 |
| `max_file_mb` | 100 | 单文件上限 |
| `max_batch_mb` | 200 | 单次剪贴的总量上限 |
| `max_files` | 50 | 单次文件数量上限 |
| `max_image_pixels` | 50000000 | 图片像素上限（防解压炸弹） |
| `file_sync` | true | 文件同步开关 |
| `sound_enabled` | true | 收到远端剪贴时是否播放提示音 |
| `cache_max_mb` | 1024 | 接收临时文件缓存上限，超出清理旧事件目录 |
| `mdns_enabled` | true | mDNS 自动发现开关 |
| `peers` | `[]` | 手动直连地址列表 |
| `log_level` | info | `RUST_LOG` 可覆盖，如 `RUST_LOG=debug` |

**限流规则：** 文本、图片、单文件、总量任一超限，整次剪贴事件不同步，不做部分同步。文件在读取前先用元数据判断大小。

## 接收文件

文件落在系统缓存目录：

- macOS: `~/Library/Caches/LanPaster/incoming/<事件目录>/`
- Linux: `~/.cache/LanPaster/incoming/...`
- Windows: `%LOCALAPPDATA%\LanPaster\cache\incoming\...`

接收成功后会把本地临时文件列表写入剪贴板，可以直接在 Finder / Explorer / 文件管理器中粘贴。每次成功接收文件后会检查 `cache_max_mb`，按目录时间删除旧事件目录，最新一次文件剪贴会保留。程序启动时也会清空旧接收目录。

## 安全说明

- PSK 只本地保存，不通过 mDNS 或网络明文传输。
- 配置文件和接收目录权限分别设为 0600 / 0700（Unix）。
- 接收端不自动打开文件，不保留可执行 / setuid 权限。
- 自用工具，未做系统级密钥链存储、证书固定、审计日志等。

## 代码结构

- `src/main.rs`：CLI、启动流程
- `src/config.rs`：配置、默认值、PSK/设备 ID
- `src/clipboard.rs`：剪贴板轮询、图片格式选择、文件元数据、远端写入
- `src/protocol.rs`：消息定义、bincode 编解码、chunk 大小
- `src/crypto.rs`：Noise 握手与加密帧
- `src/net.rs`：TCP 监听、拨号、连接管理、文件/图片/文本传输、限额校验
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

## 已验证

在 macOS arm64 上实测通过：

- 文本、JPEG 图片、单文件、多文件同步
- 两个守护进程通过 mDNS 自动发现并稳定连接
- 两个守护进程通过手动 `peers` 直连
- 超过 `max_file_mb` 的文件被拒绝，客户端收到 Reject
- PSK 不一致时握手失败
- 原生托盘启动、设备列表更新、菜单事件路径
- 配置面板独立进程启动
- 提示音命令、开机自启写入/移除
- 同一配置第二个实例被拒绝
- 修改 `max_text_kb` 后无需重启立即生效
- `clipboard-rs` 平台 watcher 在 macOS 上生效
- 两次 700KB 文件接收后只保留最新事件目录

资源占用（release，macOS arm64 实测）：

- 无界面模式：约 18–20 MB，空闲 CPU 接近 0
- 带原生托盘：约 80 MB（AppKit / NSStatusItem 基线）
- 配置面板进程：约 125 MB，仅在打开期间存在

## 下一步

1. Windows / Linux 实机验证
2. `.app` / Windows exe / Linux AppImage 打包
3. 配置热加载扩展到端口 / PSK / mDNS / 手动节点
4. 剪贴历史、设备管理
5. 目录同步 / 断点续传
