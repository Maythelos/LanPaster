# LanPaster

局域网剪贴板同步工具的第一个可运行版本（核心守护进程）。

当前版本先跑通同步内核：**文本 / 图片 / 文件** 三类剪贴内容、PSK 加密连接、mDNS 自动发现、手动 IP 直连、大小阈值、文件临时落地。托盘、配置面板、提示音、开机自启等桌面层功能尚未接入。

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

## 尚未实现

- 系统托盘 / 原生菜单
- Slint 配置面板
- 收到剪贴的提示音
- 开机自启、单实例锁
- 配置热加载
- 剪贴历史、设备列表 UI
- Linux GNOME Wayland 的原生后台剪贴板监听（Mutter 不实现 data-control；目前只能依赖 XWayland 或桌面扩展）
- 目录、符号链接、断点续传（一期仅常规文件）

## 构建

需要 Rust stable（本项目在 macOS arm64 上验证）。

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

```bash
./target/release/lanpaster run
```

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
| `cache_max_mb` | 1024 | 接收临时文件缓存上限（当前版本未强制清理，见下方说明） |
| `mdns_enabled` | true | mDNS 自动发现开关 |
| `peers` | `[]` | 手动直连地址列表 |
| `log_level` | info | `RUST_LOG` 可覆盖，如 `RUST_LOG=debug` |

**限流规则：** 文本、图片、单文件、总量任一超限，整次剪贴事件不同步，不做部分同步。文件在读取前先用元数据判断大小。

## 接收文件

文件落在系统缓存目录：

- macOS: `~/Library/Caches/LanPaster/incoming/<事件目录>/`
- Linux: `~/.cache/LanPaster/incoming/...`
- Windows: `%LOCALAPPDATA%\LanPaster\cache\incoming\...`

接收成功后会把本地临时文件列表写入剪贴板，可以直接在 Finder / Explorer / 文件管理器中粘贴。当前版本在启动时清空旧接收目录；`cache_max_mb` 的自动清理尚未实现，后续补上。

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
- `src/client.rs`：`send` / `send-file` / `send-files` / `send-image` 调试客户端

## 已验证

在 macOS arm64 上实测通过：

- 单实例文本、JPEG 图片、单文件、多文件同步
- 两个守护进程通过 mDNS 自动发现并稳定连接
- 两个守护进程通过手动 `peers` 直连
- 超过 `max_file_mb` 的文件被拒绝，客户端收到 Reject
- PSK 不一致时握手失败
- 空闲内存约 18 MB（release、无托盘），空闲 CPU 接近 0%

## 下一步

1. 接入 Slint `SystemTrayIcon` / 原生托盘菜单
2. 配置面板（设备列表、PSK、阈值、文件同步开关）
3. 提示音
4. 开机自启、单实例锁
5. 剪贴板轮询改成平台变更计数或 watcher，降低图片场景 CPU
6. 接收文件缓存上限与自动清理
