# Contributing to LanPaster

Thanks for taking the time to improve LanPaster. This is a small Rust project;
keeping changes focused and easy to review is more valuable than large rewrites.

## Project status

- macOS arm64 is the primary tested platform.
- Windows and Linux code paths are implemented and packaged, but still need
  real-device testing. If you can test on either platform, please include the OS
  version and results in your PR.
- The UI text is currently Simplified Chinese. Translation help is welcome.

## Build and test

Requirements: Rust stable 1.80 or newer.

```bash
cargo build
cargo test
```

Run the app:

```bash
cargo run -- init
cargo run -- run --headless
cargo run -- run                 # tray mode, requires a desktop session
cargo run -- ui connection       # settings window
```

Platform build dependencies:

- **macOS:** Xcode Command Line Tools.
- **Linux:** `pkg-config`, ALSA, X11/xcb, Wayland, fontconfig, and freetype
  development packages. The release package is built on Ubuntu 22.04.
- **Windows:** Visual Studio Build Tools (MSVC) or a GNU toolchain.

## Code style

- The project uses Rust 2021.
- User-facing strings and most comments are Simplified Chinese; keep new content
  consistent with the surrounding area. English identifiers are fine.
- `cargo fmt --check` is not clean across the whole repository. Do not reformat
  unrelated files; only tidy the code you change.
- Avoid adding a new dependency unless it clearly pays for itself.
- Keep the config hot-reload/restart lists in sync with `AppContext::apply_config`
  when adding settings.

## Commits and pull requests

- Use `feat:` or `fix:` prefixes, matching the existing history.
- Keep one logical change per commit/PR.
- Describe what was tested and on which platform. For UI changes, a screenshot is
  helpful.
- Never commit `config.toml`, `*.trusted.toml`, private keys, logs from a real
  device, or build artifacts in `dist/`. These are ignored by `.gitignore`.

Security issues should follow [SECURITY.md](SECURITY.md) instead of a public PR
or issue.

## License

By contributing, you agree that your changes are released under the project
license, GPL-3.0-only.
