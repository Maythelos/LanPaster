#!/usr/bin/env bash
set -euo pipefail

# 生成 Release 用的 SHA256SUMS.txt。所有平台的当前版本产物都放进 dist/ 后运行；
# 自更新会按这个文件校验下载的安装包。
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST="$ROOT/dist"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"

if [[ -z "$VERSION" ]]; then
    echo "cannot read version from Cargo.toml" >&2
    exit 1
fi
if [[ ! -d "$DIST" ]]; then
    echo "dist/ does not exist; package a release first" >&2
    exit 1
fi

cd "$DIST"
shopt -s nullglob
files=(
    "LanPaster-${VERSION}-"*.dmg
    "LanPaster-${VERSION}-"*.zip
    "LanPaster-${VERSION}-"*.deb
    "LanPaster-${VERSION}-"*.tar.gz
)
if [[ ${#files[@]} -eq 0 ]]; then
    echo "no ${VERSION} release assets found in $DIST" >&2
    exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
    hash_cmd=(sha256sum)
else
    hash_cmd=(shasum -a 256)
fi

LC_ALL=C printf '%s\n' "${files[@]}" | sort | while IFS= read -r file; do
    "${hash_cmd[@]}" "$file"
done > SHA256SUMS.txt

echo "SHA256SUMS.txt:"
cat SHA256SUMS.txt
