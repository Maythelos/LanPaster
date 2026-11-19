# AGENTS.md

## 项目概览

LanPaster 是 Rust 写的局域网剪贴板同步工具（文本 / 图片 / 文件），托盘常驻，设置窗口用 Slint。单 crate，无 workspace，无 Node / 前端工具链。

- 唯一实机验证平台是 macOS arm64；Windows / Linux 代码路径已写但未实测，不要当成已验证功能。
- `README.md` 是当前行为的权威说明。根目录的 `LanPaster … 需求与技术方案文档.md` 是早期设计稿（例如还写着 arboard，实际用 clipboard-rs），与代码冲突时以 `Cargo.toml` / `src/` 为准。
- 用户可见文案和注释都是中文，新增内容保持一致。

## 常用命令

- 构建：`cargo build`；发布：`cargo build --release`。
- 测试：`cargo test`；单测都是 `src/` 内联 `#[cfg(test)]`，没有集成测试。跑单个：`cargo test pairing::tests::approve_persists_trusted_peer`。
- 本仓库没有 CI、没有 clippy 配置，且当前 `cargo fmt --check` 不通过。不要全仓库跑 `cargo fmt`（会产生无关 diff），只整理自己改的代码。
- 启动：`./target/release/lanpaster init`，然后 `run`（托盘）或 `run --headless`（也可用 `LANPASTER_HEADLESS=1`）。
- 设置窗口：`./target/release/lanpaster ui [connection|local]`。
- 同机联调两个实例：用不同配置和不同端口，例如
  `LANPASTER_CONFIG=/tmp/lp-a.toml ./target/release/lanpaster run --headless`。
  单实例锁按配置文件路径计算，同配置的第二个 `run` 会直接退出。
- 手动推送调试：`send <host:port> [text]`、`send-file`、`send-files`、`send-image`。

## OpenIDEA MCP（仅当工具可用）

工具命名空间可能是 `openidea` 或 `OpenIDEA`，能力相同。它接的是 IDE 项目索引，语义检索比纯文本 grep 准；简单查找仍直接用 `grep` / `glob` / `read`。

- 找符号 / 调用点：`search_symbol`、`find_references`、`get_symbol_info`；跨文件改符号名用 `rename_refactoring`，不要手工替换。
- 普通检索：`search_text` / `search_regex` / `search_file`，结果自带行列号。
- 改完 Rust 文件用 `check_files` 或 `build_project` 验证（会跑 rust-analyzer / cargo check）；`get_project_problems` 只是 Problems 面板快照，可能滞后。
- 用户说“当前文件”“选中的这段”时，先 `get_current_file` 读取选区，不要猜。

## 关键结构

- `src/main.rs`：CLI 与启动流程，`start_services` 组装剪贴板、配对、mDNS 和监听任务。
- `src/net.rs`：核心网络层（监听、拨号去重、连接任务、文件/图片/文本传输、限额校验）。
- `src/clipboard.rs`：本地剪贴板监听与远端内容写入；`src/protocol.rs`：bincode 消息，`PROTOCOL_VERSION = 2`，`MAX_FRAME = 60_000`、`CHUNK_SIZE = 48 * 1024`（调整 chunk 时不要超过帧上限）。
- `src/pairing.rs`：首次连接与 `*.trusted.toml` 持久化。
- `src/tray.rs`：winit 托盘事件循环，必须在主线程运行。
- `src/ui.rs` + `ui/app.slint`：设置窗口；Slint 由 `build.rs` 编译期生成，Cupertino 风格，没有独立前端构建步骤。

## 必须知道的机制

- 托盘和设置窗口是**两个进程**：托盘通过 `current_exe ui <panel>` 拉起窗口，窗口关闭即退出。只有 `ui` 会把 macOS 激活策略切成 Regular（显示 Dock 图标）；`run` 是 Accessory（`LSUIElement`，无 Dock 图标）。
- 设置窗口只负责写 `config.toml`；守护进程每秒比对 mtime 后热加载。可热加载：各大小阈值、`file_sync`、`sound_enabled`、`poll_interval_ms`。仍需重启：`listen_port`、`mdns_enabled`、`peers`、`device_id`、设备密钥。`AppContext::apply_config` 会返回需重启字段，新增配置项时同步维护。
- 信任文件是配置文件同目录的 `<stem>.trusted.toml`（默认 `config.trusted.toml`），按 mtime 自动重载；删除信任后守护进程会在 1 秒内断开对应连接并通过 `Unpair` 通知对端。被移除的设备写入 `revoked`，不会自动重新配对。
- 限额是**整次事件拒绝**：文本、单文件、单次总量、文件数量任一超限，整次剪贴都不同步，不做部分同步。
- Linux 剪贴板把文件报告为 `file://` URI，macOS / Windows 报告本地路径；`read_files` 统一走 `file_entry_to_path` 转换，改文件剪贴逻辑时不要绕过它。
- 托盘图标是编译期 `include_bytes!` 的 PNG（`assets/tray-*.png`）。同名 SVG 只是设计源，仓库里没有生成脚本，改 SVG 不会生效，必须更新 PNG 后重新编译。
- 首次运行会生成设备身份并写入配置；不要在多台设备间复制同一份 `config.toml`。设置窗口保存配置时会重建临时文件并 rename，同时设 0600 权限。

## 打包

- macOS：`./scripts/package-macos.sh`，产物在 `dist/`（已 gitignore）。默认构建 Universal（缺少 x86_64 target 会自动安装），使用 ad-hoc 签名；`ONLY_ARM64=1` 可只构建本机架构，适合快速验证。脚本末尾会跑 `lsregister` 刷新 Finder 图标缓存，改图标后不要绕过。
- Linux：`./scripts/package-linux.sh`，需要 Docker（`DOCKER` 环境变量可覆盖，默认 `~/.docker/bin/docker`），在 ubuntu 22.04 容器里构建 deb / tar.gz。
- 版本号唯一来源是 `Cargo.toml` 的 `version`，打包文件名从它读取，发版时只改这里。

## 提交

- 现有历史使用 `feat:` / `fix:` 前缀，沿用该风格。
