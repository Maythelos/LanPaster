#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DOCKER="${DOCKER:-$HOME/.docker/bin/docker}"
if ! "$DOCKER" info >/dev/null 2>&1; then
    echo "Docker daemon is not running. Start Docker Desktop first." >&2
    exit 1
fi

"$DOCKER" build --platform linux/amd64 \
    -f packaging/linux/Dockerfile \
    -t lanpaster-linux-builder .

"$DOCKER" run --rm --platform linux/amd64 \
    -v "$ROOT":/src \
    -v lanpaster-cargo-registry:/root/.cargo/registry \
    -v lanpaster-target:/build/target \
    -e CARGO_TARGET_DIR=/build/target \
    lanpaster-linux-builder \
    bash /src/packaging/linux/build.sh
