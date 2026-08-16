#!/usr/bin/env bash
# Build the pluto73-cross image. Run once (and again only if the Dockerfile changes).
#
# The Linaro toolchain tarball is downloaded automatically into cross-compile/ on first run (checksum-verified) from ARM's Legacy Linaro GNU Toolchains archive; alternatively pass its path as the first argument.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARBALL="${1:-$HERE/gcc-linaro-7.5.0-2019.12-x86_64_arm-linux-gnueabihf.tar.xz}"

TARBALL_URL="https://developer.arm.com/-/cdn-downloads/permalink/legacy-linaro-gnu-toolchains/7.5-2019.12/gcc-linaro-7.5.0-2019.12-x86_64_arm-linux-gnueabihf.tar.xz"
TARBALL_SHA256="abf877f021c5f094d396bac4d842ed6f13aecbf4c477fc5825cf2d8b1fe3ef22"

if [[ ! -f "$TARBALL" ]]; then
  echo "==> toolchain tarball not found, downloading 105 MB to $TARBALL"
  mkdir -p "$(dirname "$TARBALL")"
  curl -fL --progress-bar -o "$TARBALL.part" "$TARBALL_URL"
  mv "$TARBALL.part" "$TARBALL"
fi

# Verfiy downloaded toolchain
if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL_SHA256="$(sha256sum "$TARBALL" | cut -d' ' -f1)"
else
  ACTUAL_SHA256="$(shasum -a 256 "$TARBALL" | cut -d' ' -f1)"
fi
if [[ "$ACTUAL_SHA256" != "$TARBALL_SHA256" ]]; then
  echo "error: checksum mismatch for $TARBALL" >&2
  echo "  expected $TARBALL_SHA256" >&2
  echo "  got      $ACTUAL_SHA256" >&2
  echo "delete the file and re-run to re-download, or see the header of this script" >&2
  exit 1
fi

# Assemble a temp context so the 105 MB tarball never has to live in the repo.
CTX="$(mktemp -d)"
trap 'rm -rf "$CTX"' EXIT
cp "$HERE/Dockerfile" "$CTX/"
cp -R "$HERE/libs" "$CTX/"
# Hardlink when possible (same volume) to avoid copying 105 MB.
ln "$TARBALL" "$CTX/$(basename "$TARBALL")" 2>/dev/null || cp "$TARBALL" "$CTX/"

docker build --platform linux/amd64 \
  --build-arg "TOOLCHAIN_TARBALL=$(basename "$TARBALL")" \
  -t pluto73-cross:latest "$CTX"

echo "==> image pluto73-cross:latest ready; build the app with cross-compile/make.sh app"
