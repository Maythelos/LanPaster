# LanPaster

[English](README.md) · [简体中文](README.zh-CN.md) · **日本語**

[![Latest release](https://img.shields.io/github/v/release/Maythelos/LanPaster?label=release)](https://github.com/Maythelos/LanPaster/releases/latest)
[![License: GPL-3.0-only](https://img.shields.io/badge/license-GPL--3.0--only-blue)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey)

LanPaster は、同じ LAN 上のマシン間でクリップボードを同期する小さな常駐型
トレイアプリです。**テキスト、画像、ファイル**に対応し、暗号化された TCP 接続で
ピア同士を直接つなぎます。クリップボードの内容がクラウドへ送られることはありません。

> **プロジェクトの状態：**実機で十分に検証できているのは macOS arm64 のみです。
> Windows / Linux のコードパスは実装・パッケージ済みですが、実機での検証はこれ
> からです。不具合報告や検証結果を歓迎します。

## ダウンロード

最新版は **[GitHub Releases](https://github.com/Maythelos/LanPaster/releases/latest)** から
ダウンロードできます。

| プラットフォーム | アーキテクチャ | パッケージ |
| --- | --- | --- |
| macOS 13+ | Universal（arm64 + x86_64） | `.dmg`（推奨）または `.zip` |
| Windows 10/11 | x86_64 | `.zip`：`lanpaster.exe`（GUI）+ `lanpaster-console.exe` |
| Linux | x86_64 | `.deb`（Ubuntu/Debian）または `.tar.gz` |

Release ページには `SHA256SUMS.txt` も用意されています。

<details>
<summary>インストール手順</summary>

**macOS** — DMG を開き、LanPaster を「アプリケーション」へドラッグして起動します。
ad-hoc 署名のため、初回起動時に Gatekeeper に止められる場合があります。アプリを
右クリックして「開く」を選ぶか、次を実行してください：

```bash
xattr -dr com.apple.quarantine /Applications/LanPaster.app
```

**Windows** — ZIP を解凍して `lanpaster.exe` を実行します。コード署名がないため、
SmartScreen の警告が表示された場合は「詳細情報」→「実行」を選んでください。
`lanpaster-console.exe` はコンソール付きの同じアプリで、`init` や `version`、
ログ確認に便利です。

**Linux** — Ubuntu/Debian の場合：

```bash
sudo apt install ./LanPaster-<version>-linux-amd64.deb
```

tar.gz を展開して `./usr/bin/lanpaster` を実行する場合は、Release ページに記載の
ランタイム依存パッケージを入れてください。Linux バイナリは Ubuntu 22.04
（glibc 2.35+）でビルドしています。

</details>

## 主な機能

- **3 種類のクリップボード** — UTF-8 テキスト、画像、ファイルリスト。
- **画像形式の保持** — クリップボードが提供する元のエンコード（PNG、JPEG、WebP、
  GIF、HEIC、TIFF）を優先し、取得できない場合は PNG に変換します。
- **ストリーミング転送** — マニフェスト交渉、チャンク転送、SHA-256 検証。受信側は
  まず一時キャッシュへ書き、全ファイルが成功してからクリップボードへ反映します。
- **明示的な信頼** — Noise `XX` ハンドシェイクと永続的な X25519 デバイス ID。
  初回接続はトレイから手動で承認する必要があります。信頼済みピアはローカルに
  保存され、削除したピアも記録されます。
- **検出のみ、自動接続なし** — mDNS はデバイスの検出だけを行います。トレイで
  デバイスを選んだときに接続します。`host:port` の手動ピアも利用できます。
- **ネイティブなトレイ** — 同期のオン/オフ、検出済み/オンラインのデバイス、接続
  リクエスト、クリップボード履歴、設定、自動起動、テスト音、終了など。macOS では
  テンプレート画像のメニューバーアイコンを使い、Dock には表示しません。
- **小さな設定センター** — 「接続 / ローカル」の 2 ページを持つ Slint ウィンドウ。
  設定画面は別プロセスで、閉じると終了します。トレイアイコンを再度クリックした
  場合は、開いているウィンドウを再利用します。
- **実用的な既定値** — サイズ/件数の上限、受信ファイルキャッシュの整理、メモリ
  上の履歴、単一インスタンス、ほとんどの設定のホットリロード。
- **手動 GitHub セルフアップデート** — トレイメニューの「アップデートを確認」を
  押したときだけ最新 Release を問い合わせ、`SHA256SUMS.txt` で検証します。実際の
  置き換えは、もう一度確認してから行います。

## 仕組み

1. **検出** — mDNS でデバイス ID、名前、プロトコルバージョンを通知します。検出
   だけでは接続を開始しません。
2. **ペアリング** — 永続的な X25519 ID を使って Noise `XX` セッションを確立します。
   受信側がトレイで承認すると、フィンガープリントが
   `<設定ファイル>.trusted.toml` に保存されます。
3. **同期** — クリップボードの変更を暗号化メッセージにして接続中のピアへ直接
   送ります。ファイルは先にマニフェストを交渉し、チャンク転送と SHA-256 検証を
   行います。
4. **受信** — ファイルはイベントごとのキャッシュディレクトリへ書き込み、すべて
   成功した時点でファイルリストをローカルのクリップボードへ設定します。実行
   ビットや setuid ビットは保持せず、受信ファイルを自動で開くこともありません。

## クイックスタート

1. 同じ LAN 上の 2 台で LanPaster を起動します。
2. トレイメニュー →「LAN デバイスをスキャン」→ 相手のデバイスをクリック。
3. 受信側のトレイメニュー →「接続リクエスト」→「承認」。
4. 承認後は再接続時に確認されず、クリップボードの変更が自動で同期されます。

mDNS が使えない場合は手動ピアを設定できます：

```toml
mdns_enabled = false
peers = ["192.168.1.20:39393", "192.168.1.21:39393"]
```

サーバーやデバッグ用途ではトレイなしで起動できます：

```bash
lanpaster run --headless
# または LANPASTER_HEADLESS=1 lanpaster run
```

設定画面だけを開くこともできます：

```bash
lanpaster ui connection   # デバイス名、フィンガープリント、ポート、mDNS、手動ピア
lanpaster ui local        # ポーリング、上限、ファイル同期、音、履歴
```

## 設定

既定の設定ファイル：

| プラットフォーム | パス |
| --- | --- |
| macOS | `~/Library/Application Support/LanPaster/config.toml` |
| Linux | `~/.config/LanPaster/config.toml` |
| Windows | `%APPDATA%\LanPaster\config.toml` |

信頼済みピアのファイルは設定ファイルと同じディレクトリの
`<設定ファイル名>.trusted.toml`（既定では `config.trusted.toml`）です。
`LANPASTER_CONFIG=/path/foo.toml` を指定した場合は
`/path/foo.trusted.toml` になります。

| キー | 既定値 | 説明 |
| --- | ---: | --- |
| `device_name` | ホスト名 | ピアに表示する名前 |
| `device_id` | 自動生成 | デバイス固有 ID。複数デバイスで再利用しない |
| `private_key` / `public_key` | 自動生成 | X25519 の ID。コピー・共有しない |
| `listen_port` | `39393` | TCP 待ち受けポート |
| `poll_interval_ms` | `500` | クリップボードのポーリング間隔（ms） |
| `max_text_kb` | `1024` | 1 イベントあたりのテキスト上限 |
| `max_file_mb` | `100` | 1 ファイル/画像あたりの上限（エンコード後のバイト数） |
| `max_batch_mb` | `200` | 1 イベントあたりの合計上限 |
| `max_files` | `50` | 1 イベントあたりのファイル数上限 |
| `file_sync` | `true` | ファイル同期を有効化 |
| `sound_enabled` | `true` | リモート受信時に通知音を鳴らす |
| `cache_max_mb` | `1024` | 受信ファイルキャッシュの上限 |
| `history_minutes` | `10` | メモリ上の履歴を保持する分数。`0` で無効 |
| `history_limit` | `20` | 履歴の最大件数。`0` で無効 |
| `mdns_enabled` | `true` | mDNS 検出を有効化 |
| `peers` | `[]` | 手動ピア（`host:port`）。起動時に自動でダイヤルされます |
| `log_level` | `info` | ログレベル。`RUST_LOG` で上書き可能 |

**上限はイベント単位です。** テキスト、単一ファイル、合計サイズ、ファイル数の
いずれかが上限を超えると、そのクリップボードイベント全体を拒否します。部分同期は
行いません。画像は 1 ファイルとして扱い、ピクセル数ではなくエンコード後の
バイト数で制限します。

**ホットリロード** — 次の設定は約 1 秒以内に反映されます：

`max_text_kb`、`max_file_mb`、`max_batch_mb`、`max_files`、`file_sync`、
`sound_enabled`、`poll_interval_ms`、`history_minutes`、`history_limit`。

`listen_port`、`mdns_enabled`、`peers`、`device_id`、デバイス鍵の変更には
再起動が必要です。

### アップデート

アップデートは完全に手動です。トレイメニューの「アップデートを確認」を押した
ときだけ、最新の GitHub Release を問い合わせます。起動時やバックグラウンドで
自動確認はしません。新しいバージョンがあればメニューが「vX に更新…」に変わり、
もう一度クリックするまでダウンロードもインストールも行いません。
`lanpaster update check` はコマンドラインから確認だけを行います。

macOS では `.app` が「アプリケーション」などユーザーが書き込める場所にある
必要があります。App Translocation と読み取り専用ボリュームは拒否されます。
Linux の `.deb` インストールは dpkg が管理するため自己更新しません。`apt` か
Release ページを使用してください。チェックサムはダウンロードの破損・改変を
防ぎますが、コード署名の代替ではありません。

## コマンドライン

| コマンド | 説明 |
| --- | --- |
| `lanpaster` / `lanpaster run` | トレイ常駐デーモンを起動 |
| `lanpaster run --headless` | トレイなしで起動（サーバー / デバッグ） |
| `lanpaster init` | 設定を作成し、デバイス ID / フィンガープリントを表示 |
| `lanpaster ui [connection\|local]` | 設定画面を開く |
| `lanpaster beep` | 通知音を鳴らす |
| `lanpaster autostart <enable\|disable\|status>` | 自動起動を管理 |
| `lanpaster update [check]` | GitHub Releases を確認（インストールはしない） |
| `lanpaster config` | 現在の設定を表示 |
| `lanpaster version` | バージョンを表示 |
| `lanpaster help` | CLI ヘルプを表示 |

環境変数：

- `LANPASTER_CONFIG=/path/to/config.toml` — 設定ファイルのパスを変更。
- `LANPASTER_HEADLESS=1` — `run --headless` と同じ。
- `RUST_LOG=debug` — 設定のログレベルを上書き。

自動起動は既定の設定パスを使用します。`LANPASTER_CONFIG` を使う場合は、ログイン
項目を手動で調整してください。

## ソースからビルド

必要なもの：

- Rust stable **1.80+**
- macOS：Xcode Command Line Tools
- Windows：Visual Studio Build Tools（MSVC）または GNU ツールチェーン
- Linux：`pkg-config`、ALSA、X11/xcb、Wayland、fontconfig、freetype の開発パッケージ

```bash
cargo build --release
cargo test
./target/release/lanpaster init
```

`Cargo.toml` の `version` がバージョンの唯一の情報源で、パッケージ名もここから
生成されます。

### パッケージング

- **macOS** — `./scripts/package-macos.sh` で Universal `.app`、DMG、ZIP を
  `dist/` に生成します。`ONLY_ARM64=1` を付けると arm64 のみの高速ビルドになり
  ます。ad-hoc 署名です。
- **Linux** — `./scripts/package-linux.sh` が Ubuntu 22.04 の Docker コンテナ内で
  `.deb` と `.tar.gz` を生成します。`docker` が `PATH` にない場合は `DOCKER=` で
  指定してください。
- **Windows** — `./scripts/package-windows.sh` が LLVM-MinGW
  (`x86_64-pc-windows-gnullvm`) でクロスコンパイルし、GUI 版と console 版の両方
  を含む ZIP を生成します。`LLVM_MINGW=/path/to/llvm-mingw` を指定するか、
  `x86_64-w64-mingw32-clang` を `PATH` に入れてください。Windows 上での
  `cargo build --release` も開発用に使えますが、ネイティブビルドの既定は
  console サブシステムです。
- **チェックサム** — 現在のバージョンの全プラットフォームの成果物を `dist/` に
  揃えたら、`./scripts/checksums.sh` を実行して `SHA256SUMS.txt` を再生成します。
  セルフアップデートは、このファイルや該当エントリがない Release を拒否します。

## セキュリティとプライバシー

- クリップボードデータは**同一 LAN 上のピア間だけ**を流れます。クラウド、中継、
  アカウントはありません。
- 通信は Noise `XX` ハンドシェイクで暗号化され、デバイス ID は永続的な X25519
  鍵ペアです。
- 初回接続は手動承認が必要です。信頼済みピアはローカルの `*.trusted.toml` に
  保存されます。ピアを削除すると相手側にも通知し、取り消しを記録するため、
  mDNS によって勝手に再ペアリングされることはありません。
- Unix では設定ファイルと信頼ファイルを `0600`、受信ディレクトリを `0700` で
  作成します。
- `config.toml` には秘密鍵が含まれ、`lanpaster config` は設定全体を出力します。
  共有する前に必ず秘密鍵を伏せてください。
- 受信ファイルは自動で開かず、実行ビット / setuid ビットも保持しません。
- セルフアップデートは HTTPS でダウンロードし、Release の `SHA256SUMS.txt` で
  検証します。チェックサム以外の署名はないため、GitHub アカウントが侵害される
  と悪意あるアップデートが公開される可能性があります。
- **未実装：**OS キーチェーン、証明書ピンニング、署名付き監査ログ、ローカル
  アカウント侵害への対策。LAN を信頼するツールであり、高強度のセキュア
  メッセンジャーではありません。
- クリップボード履歴はメモリ上に平文で保持されます。パスワードなどをコピーした
  後は「履歴をクリア」を使ってください。

セキュリティ上の問題を見つけた場合は、公開 issue ではなく GitHub の Security
Advisory から非公開で報告してください。詳細は [SECURITY.md](SECURITY.md) を
参照してください。

## プラットフォーム対応

| プラットフォーム | 状態 | 備考 |
| --- | --- | --- |
| macOS 13+ arm64 | ✅ 検証済み | 主な開発環境（Apple Silicon） |
| macOS x86_64 | ⚠️ ビルド済み | Universal バイナリに含まれるが、実行検証は少なめ |
| Windows 10/11 x64 | ⚠️ 実装済み | パッケージ済み。実機検証が必要 |
| Linux x86_64 | ⚠️ 実装済み | Ubuntu 22.04 でパッケージ済み。実機検証が必要 |

既知の制限：GNOME Wayland では Mutter が data-control プロトコルを提供しない
ため、ネイティブなバックグラウンド監視には制限があります。XWayland やデスクトップ
拡張で動作する場合があります。これは上流側の制限です。

## プロジェクト構成

```text
src/
  main.rs        CLI、起動処理、サービスの組み立て
  config.rs      設定、既定値、デバイス ID
  clipboard.rs   クリップボード監視、画像形式、ファイル処理
  protocol.rs    暗号化メッセージ定義とフレーミング
  crypto.rs      Noise ハンドシェイク
  net.rs         TCP 待ち受け、ダイヤル、接続、転送
  pairing.rs     初回承認、信頼済みピアの永続化
  mdns.rs        mDNS 登録・検出
  tray.rs        ネイティブトレイとメニュー
  ui.rs          Slint 設定ウィンドウのプロセス
  history.rs     メモリ上のクリップボード履歴
  cache.rs       受信ファイルキャッシュの整理
  sound.rs       通知音
  autostart.rs   自動起動
  instance.rs    単一インスタンスロック
  updater.rs     GitHub Releases セルフアップデート
ui/app.slint     設定画面のレイアウト
assets/          アプリアイコンとトレイアイコン
packaging/       各プラットフォームのパッケージング
scripts/         リリース用スクリプト
```

## 制限とロードマップ

- クリップボード履歴はメモリのみで、プロセス終了時に消えます。
- ディレクトリ、シンボリックリンク、レジューム可能な転送は未対応です。
- クリップボード履歴は永続化されません。信頼済みピア情報は TOML に保存されます。
- Windows / Linux はさらに実機検証が必要です。
- UI 文言は現在簡体字中国語のみです。他言語の翻訳を歓迎します。
- Linux GNOME Wayland のバックグラウンド監視には制限があります。
- ネイティブインストーラー / パッケージマネージャー配布は未対応です。

## トラブルシューティング

- **デバイスが見つからない** — 同じサブネットにいること、mDNS が有効なこと、
  ファイアウォールが UDP 5353 と設定した TCP ポートを許可していることを確認して
  ください。mDNS が使えない場合は `peers = ["host:port"]` を設定します。
- **バージョン不一致** — プロトコルバージョンが違うデバイスは「検出済みデバイス」
  に互換性なしと表示されます。両方を更新してください。
- **ポートが使用中** — 設定の `listen_port` を変更して再起動してください。
- **接続が承認されない** — トレイの「接続リクエスト」を確認してください。新しい
  デバイス ID には毎回承認が必要です。
- **デバイス ID をコピーしない** — `config.toml` を別のマシンへコピーしないで
  ください。同じ秘密鍵を持つ別デバイスとして扱われます。
- **受信ファイルの場所** — macOS：`~/Library/Caches/LanPaster/incoming`、
  Windows：`%LOCALAPPDATA%\LanPaster\cache\incoming`、Linux：
  `~/.cache/LanPaster/incoming`。
- **macOS Gatekeeper / Windows SmartScreen** — Release のバイナリは有料証明書で
  署名・公証されていません。上記の右クリック「開く」または「詳細情報 → 実行」を
  使ってください。

## コントリビュート

issue と pull request を歓迎します。ビルド / テスト手順、プラットフォームの
状態、コミット規約は [CONTRIBUTING.md](CONTRIBUTING.md) を参照してください。

## ライセンス

LanPaster は **GNU General Public License v3.0 only**（`GPL-3.0-only`）で公開
されています。詳細は [LICENSE](LICENSE) を参照してください。
