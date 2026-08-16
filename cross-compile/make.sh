#!/usr/bin/env bash
# Run a pluto73 Makefile target inside the cross-build container.
#
#   cross-compile/make.sh app               # cross-compile the app for the Pluto
#   cross-compile/make.sh app-diagnostics   # ditto, diagnostics binary
#
set -euo pipefail

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

exec docker run --rm --platform linux/amd64 \
  -v "$APP_DIR:/work" \
  -v pluto73-cargo-registry:/usr/local/cargo/registry \
  -w /work \
  pluto73-cross:latest \
  make "${@:-app}" \
    CROSS_COMPILE_PATH=/opt/toolchain/bin \
    TOOLCHAIN_DIR=/opt/toolchain \
    SYSROOT=/opt/toolchain/arm-linux-gnueabihf/libc
