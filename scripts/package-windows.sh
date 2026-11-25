#!/usr/bin/env bash
set -euo pipefail

# 在 macOS 上使用 LLVM-MinGW 交叉编译 Windows x64 发布包。
# 依赖 rustup target x86_64-pc-windows-gnullvm 和 LLVM-MinGW。
# 可用 LLVM_MINGW=/path/to/llvm-mingw 指定工具链，否则要求它在 PATH 里。

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
TARGET="x86_64-pc-windows-gnullvm"
MINGW_CLANG="x86_64-w64-mingw32-clang"
PKG_NAME="LanPaster-${VERSION}-windows-x86_64"
PKG="$ROOT/dist/$PKG_NAME"
ZIP="$ROOT/dist/${PKG_NAME}.zip"
TMP_TARGET="$ROOT/target/windows-package"

if [[ -n "${LLVM_MINGW:-}" ]]; then
    export PATH="$LLVM_MINGW/bin:$PATH"
fi

if ! command -v "$MINGW_CLANG" >/dev/null 2>&1; then
    echo "LLVM-MinGW not found: $MINGW_CLANG is not in PATH." >&2
    echo "Download it from https://github.com/mstorsjo/llvm-mingw/releases and set LLVM_MINGW." >&2
    exit 1
fi

if ! rustup target list --installed | grep -qx "$TARGET"; then
    echo "Installing Rust target $TARGET..."
    rustup target add "$TARGET"
fi

export CC_x86_64_pc_windows_gnullvm="$MINGW_CLANG"
export CXX_x86_64_pc_windows_gnullvm="${MINGW_CLANG}++"
export AR_x86_64_pc_windows_gnullvm="llvm-ar"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER="$MINGW_CLANG"
export CARGO_TARGET_DIR="$TMP_TARGET"

rm -rf "$PKG" "$ZIP"
mkdir -p "$PKG" "$(dirname "$ZIP")"

echo "Building console executable for $TARGET..."
cargo build --release --target "$TARGET"
cp "$TMP_TARGET/$TARGET/release/lanpaster.exe" "$PKG/lanpaster-console.exe"

echo "Building GUI executable for $TARGET..."
cargo rustc --release --target "$TARGET" -- -C link-arg=-mwindows
cp "$TMP_TARGET/$TARGET/release/lanpaster.exe" "$PKG/lanpaster.exe"

sed "s/__VERSION__/${VERSION}/g" "$ROOT/packaging/windows/README.txt" > "$PKG/README.txt"

(
    cd "$ROOT/dist"
    zip -r -q "$ZIP" "$PKG_NAME"
)

echo "Windows package: $ZIP"
ls -lh "$ZIP"
file "$PKG/lanpaster.exe" "$PKG/lanpaster-console.exe"
