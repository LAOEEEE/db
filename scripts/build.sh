#!/usr/bin/env bash
# Cross-compile the server for Raspberry Pi (aarch64) inside WSL/Linux.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

TARGET="aarch64-unknown-linux-gnu"
# Separate target dir so WSL/Linux artifacts never mix with Windows builds.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target/wsl}"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="aarch64-linux-gnu-gcc"

if ! command -v aarch64-linux-gnu-gcc >/dev/null 2>&1; then
    echo "error: aarch64-linux-gnu-gcc not found." >&2
    echo "install with: sudo apt install gcc-aarch64-linux-gnu" >&2
    exit 1
fi

echo "==> cargo test (native)"
cargo test

echo "==> cargo clippy"
cargo clippy -- -D warnings

echo "==> release build for ${TARGET}"
cargo build --release --target "${TARGET}" -p scratch-card-server

# 版本号从 workspace 的 Cargo.toml 读取（单一来源），也可用 VERSION=... 环境变量覆盖
VERSION="${VERSION:-$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/')}"

DIST="$ROOT/dist"
echo "==> bundling into ${DIST}"
rm -rf "$DIST"
mkdir -p "$DIST/web/assets" "$DIST/config"
cp "$CARGO_TARGET_DIR/${TARGET}/release/scratch-card-server" "$DIST/"
cp web/index.html "$DIST/web/"
cp web/assets/* "$DIST/web/assets/"
cp config/game.json "$DIST/config/"
cp systemd/scratch-card.service "$DIST/"

BIN_SIZE=$(du -h "$DIST/scratch-card-server" | cut -f1)
WEB_SIZE=$(du -sh "$DIST/web" | cut -f1)
echo "==> done. binary=${BIN_SIZE}, web=${WEB_SIZE}"

# ---- 打包发行版 tar.gz，输出到 dist/ ----
# 压缩包只含运行所需文件，不含发行说明
ARCHIVE="scratch-card-v${VERSION}.tar.gz"
echo "==> packaging ${ARCHIVE}"
tar -czf "$DIST/$ARCHIVE" -C "$DIST" \
    scratch-card-server web config scratch-card.service
ARCHIVE_SIZE=$(du -h "$DIST/$ARCHIVE" | cut -f1)
echo "==> archive ${ARCHIVE_SIZE} -> ${DIST}/${ARCHIVE}"
