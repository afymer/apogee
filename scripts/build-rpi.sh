#!/usr/bin/env bash
set -euo pipefail

ARCH="${1:-arm64}"

case "$ARCH" in
    arm64|aarch64)
        TARGET="export-arm64"
        OUT_DIR="./dist/arm64"
        ;;
    armv7|armhf)
        TARGET="export-armv7"
        OUT_DIR="./dist/armv7"
        ;;
    *)
        echo "Usage: $0 [arm64|armv7]"
        exit 1
        ;;
esac

mkdir -p "$OUT_DIR"

docker build \
    -f Dockerfile.cross \
    --target "$TARGET" \
    --output type=local,dest="$OUT_DIR" \
    .

echo "Build successful: $OUT_DIR/daemon"
file "$OUT_DIR/daemon" 2>/dev/null || true
