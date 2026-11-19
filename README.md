# LanPaster

**English** · [简体中文](README.zh-CN.md) · [日本語](README.ja.md)

[![Latest release](https://img.shields.io/github/v/release/Maythelos/LanPaster?label=release)](https://github.com/Maythelos/LanPaster/releases/latest)
[![License: GPL-3.0-only](https://img.shields.io/badge/license-GPL--3.0--only-blue)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey)

LanPaster is a small, native tray app that keeps the clipboard in sync across
machines on the same local network. It supports **text, images, and files**,
connects peers directly over an encrypted TCP channel, and never sends clipboard
data to a cloud service.

> **Project status:** macOS arm64 is the only platform that has been tested on
> real hardware. The Windows and Linux code paths are implemented and packaged,
> but still need broader real-device validation. Bug reports and test results are
> very welcome.

## Download

Grab the latest build from **[GitHub Releases](https://github.com/Maythelos/LanPaster/releases/latest)**.

| Platform | Architecture | Package |
| --- | --- | --- |
| macOS 13+ | Universal (arm64 + x86_64) | `.dmg` (recommended) or `.zip` |
| Windows 10/11 | x86_64 | `.zip` — `lanpaster.exe` (GUI) + `lanpaster-console.exe` |
| Linux | x86_64 | `.deb` (Ubuntu/Debian) or `.tar.gz` |

The release page also provides `SHA256SUMS.txt`.

<details>
<summary>Install notes</summary>

**macOS** — open the DMG, drag LanPaster to Applications, and launch it. The app
is ad-hoc signed, so Gatekeeper may block the first launch. Right-click the app
and choose **Open**, or run:

```bash
xattr -dr com.apple.quarantine /Applications/LanPaster.app
```

**Windows** — unzip the archive and run `lanpaster.exe`. The package is not code
signed, so SmartScreen may show a warning; choose **More info → Run anyway**.
`lanpaster-console.exe` is the same app with a console attached, useful for
`init`, `version`, and logs.

**Linux** — on Ubuntu/Debian:

```bash
sudo apt install ./LanPaster-<version>-linux-amd64.deb
```

or extract the tarball and run `./usr/bin/lanpaster` after installing the
runtime dependencies listed on the release page. The Linux binaries are built on
Ubuntu 22.04 (glibc 2.35+).

</details>

## Highlights

- **Three clipboard types** — UTF-8 text, images, and file lists.
- **Image format preservation** — prefers the original clipboard encoding
  (PNG, JPEG, WebP, GIF, HEIC, TIFF) and falls back to PNG when needed.
- **Streamed file transfer** — manifest negotiation, chunked transfer, and
  SHA-256 verification. Files are written to a temporary cache first and only
  then placed on the clipboard.
- **Explicit trust** — a Noise `XX` handshake with persistent X25519 identities;
  the first connection must be approved from the tray. Trusted peers are stored
  locally, and removed peers are remembered.
- **LAN discovery, no auto-connect** — mDNS only discovers devices; a connection
  starts when you pick a device from the tray. Manual `host:port` peers are
  supported as a fallback.
- **Native tray experience** — sync on/off, discovered/online devices,
  connection requests, clipboard history, settings, autostart, test sound, and
  quit. macOS uses a template menu-bar icon and no Dock icon.
- **Small settings center** — a single Slint window with connection and local
  preferences; it is a separate process and exits when closed.
- **Useful defaults** — size/count limits, file cache cleanup, in-memory history,
  single-instance locking, and hot reload for most settings.

## How it works

1. **Discover** — LanPaster advertises a small mDNS record with its device ID,
   name, and protocol version. Discovery never starts a connection by itself.
2. **Pair** — peers establish a Noise `XX` session using their persistent
   X25519 identities. The receiver approves the connection from the tray, and the
   fingerprint is stored in `<config>.trusted.toml`.
3. **Sync** — a clipboard change is encoded into an encrypted message and sent
   directly to connected peers. Files are negotiated first, then streamed in
   chunks and verified with SHA-256.
4. **Receive** — files land in a per-event cache directory; only after every file
   is written successfully is the file list placed on the local clipboard.
   Executable bits and setuid bits are not preserved, and received files are
   never opened automatically.

## Quick start

1. Start LanPaster on two machines on the same LAN.
2. Open the tray menu → **Scan LAN devices** → pick the other device.
3. On the receiving machine, open the tray menu → **Connection requests** →
   **Approve**.
4. After approval, the devices can reconnect without asking again. Clipboard
   changes are synchronized automatically.

Manual peers are useful when mDNS is unavailable:

```toml
mdns_enabled = false
peers = ["192.168.1.20:39393", "192.168.1.21:39393"]
```

For servers or debugging, run without a tray:

```bash
lanpaster run --headless
# or: LANPASTER_HEADLESS=1 lanpaster run
```

The settings window is also available on demand:

```bash
lanpaster ui connection   # device name, fingerprint, port, mDNS, manual peers
lanpaster ui local        # polling, limits, file sync, sound, history
```

## Configuration

Default config locations:

| Platform | Path |
| --- | --- |
| macOS | `~/Library/Application Support/LanPaster/config.toml` |
| Linux | `~/.config/LanPaster/config.toml` |
| Windows | `%APPDATA%\LanPaster\config.toml` |

The trusted-peer file is `<config-stem>.trusted.toml` next to the config file
(for example `config.trusted.toml`). If you use `LANPASTER_CONFIG=/path/foo.toml`,
the trust file is `/path/foo.trusted.toml`.

| Key | Default | Description |
| --- | ---: | --- |
| `device_name` | hostname | Name shown to peers |
| `device_id` | generated | Unique device ID; do not reuse across devices |
| `private_key` / `public_key` | generated | X25519 identity; never copy or share |
| `listen_port` | `39393` | TCP listen port |
| `poll_interval_ms` | `500` | Clipboard polling interval (milliseconds) |
| `max_text_kb` | `1024` | Maximum text size per event |
| `max_file_mb` | `100` | Maximum size per file/image (encoded bytes) |
| `max_batch_mb` | `200` | Maximum total size per clipboard event |
| `max_files` | `50` | Maximum files per event |
| `file_sync` | `true` | Enable file clipboard sync |
| `sound_enabled` | `true` | Play a sound when remote clipboard content arrives |
| `cache_max_mb` | `1024` | Received-file cache limit |
| `history_minutes` | `10` | In-memory history window in minutes; `0` disables |
| `history_limit` | `20` | Maximum history entries; `0` disables |
| `mdns_enabled` | `true` | Enable mDNS discovery |
| `peers` | `[]` | Manual peers in `host:port` form; dialed automatically on startup |
| `log_level` | `info` | Log level; `RUST_LOG` overrides it |

**Limits are per event.** If text, a single file, the total size, or the file
count exceeds a limit, the whole clipboard event is rejected; LanPaster does not
partially sync. Images count as one file item and are limited by their encoded
size, not by pixel count.

**Hot reload** — changing these settings is picked up by the running daemon
within about a second:

`max_text_kb`, `max_file_mb`, `max_batch_mb`, `max_files`, `file_sync`,
`sound_enabled`, `poll_interval_ms`, `history_minutes`, `history_limit`.

Changing `listen_port`, `mdns_enabled`, `peers`, `device_id`, or the device
keys requires a restart.

## Command line

| Command | Description |
| --- | --- |
| `lanpaster` / `lanpaster run` | Start the tray daemon |
| `lanpaster run --headless` | Run without a tray (servers / debugging) |
| `lanpaster init` | Create the config and print device identity/fingerprint |
| `lanpaster ui [connection\|local]` | Open the settings window |
| `lanpaster beep` | Play the notification sound |
| `lanpaster autostart <enable\|disable\|status>` | Manage login autostart |
| `lanpaster config` | Print the active config |
| `lanpaster version` | Print the version |
| `lanpaster help` | Show CLI help |

Environment variables:

- `LANPASTER_CONFIG=/path/to/config.toml` — use a custom config path.
- `LANPASTER_HEADLESS=1` — same as `run --headless`.
- `RUST_LOG=debug` — override the configured log level.

Autostart uses the default config path. If you run with `LANPASTER_CONFIG`,
adjust the login item yourself.

## Build from source

Requirements:

- Rust stable **1.80+**
- macOS: Xcode Command Line Tools
- Windows: Visual Studio Build Tools (MSVC) or a GNU toolchain
- Linux: `pkg-config`, ALSA, X11/xcb, Wayland, fontconfig, and freetype
  development packages

```bash
cargo build --release
cargo test
./target/release/lanpaster init
```

The version in `Cargo.toml` is the single source of truth for package names.

### Packaging

- **macOS** — `./scripts/package-macos.sh` builds a Universal `.app`, DMG, and
  ZIP in `dist/`. Set `ONLY_ARM64=1` for a faster arm64-only build. The app is
  ad-hoc signed.
- **Linux** — `./scripts/package-linux.sh` builds a `.deb` and `.tar.gz` inside
  an Ubuntu 22.04 Docker container. Set `DOCKER=` if `docker` is not on your
  `PATH`.
- **Windows** — the release ZIP is currently cross-compiled with
  LLVM-MinGW (`x86_64-pc-windows-gnullvm`) and contains both the GUI and console
  binaries. A native `cargo build --release` also works for development; it
  produces a console-subsystem executable.

## Security and privacy

- Clipboard data travels **only between peers on the LAN**; there is no cloud,
  relay, or account.
- The transport is encrypted with the Noise `XX` handshake; device identities are
  persistent X25519 key pairs.
- The first connection requires manual approval. Trusted peers are stored in a
  local `*.trusted.toml`; removing a peer also notifies the other side and records
  a revocation, so it is not silently paired again.
- On Unix, the config and trusted files are written with mode `0600`; received
  file directories are created with mode `0700`.
- `config.toml` contains the private key, and `lanpaster config` prints the full
  file. Do not share either without redacting the key first.
- Received files are never opened automatically and do not keep executable or
  setuid bits.
- **Not implemented:** OS keychain storage, certificate pinning, signed audit
  logs, or protection against a compromised local account. Treat this as a
  LAN-trusted tool, not as a hardened secure messenger.
- Clipboard history is kept in memory and contains plaintext; use **Clear
  history** after copying passwords or other sensitive data.

If you find a security issue, please use GitHub's private security advisory flow
instead of opening a public issue. See [SECURITY.md](SECURITY.md).

## Platform support

| Platform | Status | Notes |
| --- | --- | --- |
| macOS 13+ arm64 | ✅ Tested | Primary development platform; Apple Silicon |
| macOS x86_64 | ⚠️ Built | Included in the Universal binary; less runtime testing |
| Windows 10/11 x64 | ⚠️ Implemented | Packaged; needs real-device validation |
| Linux x86_64 | ⚠️ Implemented | Packaged on Ubuntu 22.04; needs real-device validation |

Known platform caveat: on GNOME Wayland, Mutter does not expose the data-control
protocol, so native background clipboard monitoring is limited. Running under
XWayland or using a desktop extension may work; this is an upstream limitation.

## Project layout

```text
src/
  main.rs        CLI, startup, and service wiring
  config.rs      Config, defaults, device identity
  clipboard.rs   Clipboard watching, image formats, file handling
  protocol.rs    Encrypted message definitions and framing
  crypto.rs      Noise handshake helpers
  net.rs         TCP listener, dialing, connections, transfers
  pairing.rs     Approval flow and trusted-peer persistence
  mdns.rs        mDNS registration/discovery
  tray.rs        Native tray and menu
  ui.rs          Slint settings window process
  history.rs     In-memory clipboard history
  cache.rs       Received-file cache cleanup
  sound.rs       Notification sound
  autostart.rs   Login autostart
  instance.rs    Single-instance lock
ui/app.slint     Settings window layout
assets/          App and tray icons
packaging/       Platform packaging files
scripts/         Release packaging helpers
```

## Limitations and roadmap

- Clipboard history is in-memory only; it is cleared when the process exits.
- Directories, symbolic links, and resumable downloads are not supported.
- Clipboard history is not persisted. Trusted-device data is stored in a plain TOML file.
- Windows/Linux need more real-device validation.
- The UI text is currently Simplified Chinese; additional translations are
  welcome.
- Linux GNOME Wayland background clipboard monitoring remains limited.
- Native installers / package-manager distribution are not set up yet.

## Troubleshooting

- **No devices discovered** — make sure both devices are on the same subnet, mDNS
  is enabled, and the firewall allows UDP 5353 and the configured TCP port. Use
  `peers = ["host:port"]` if mDNS is unavailable.
- **Version mismatch** — devices with different protocol versions are shown as
  incompatible in the discovered-device list; update both sides.
- **Port already in use** — change `listen_port` in the config and restart.
- **Connection not approved** — check the tray's **Connection requests**
  submenu; approval is required for every new identity.
- **Device identity copied** — never copy `config.toml` between machines. It
  contains the private key and would make both devices appear as the same peer.
- **Received files** — files are written under the platform cache directory
  (`~/Library/Caches/LanPaster/incoming` on macOS, `%LOCALAPPDATA%\LanPaster\cache\incoming`
  on Windows, `~/.cache/LanPaster/incoming` on Linux).
- **macOS Gatekeeper / Windows SmartScreen** — the release binaries are not
  notarized/signed with a paid certificate; use the right-click/Open or
  "More info → Run anyway" flow described above.

## Contributing

Issues and pull requests are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md)
for build/test commands, platform status, and commit conventions.

## License

LanPaster is released under the **GNU General Public License v3.0 only**
(`GPL-3.0-only`). See [LICENSE](LICENSE).
