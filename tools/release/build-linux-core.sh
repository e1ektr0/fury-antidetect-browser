#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# End-to-end script to build and package Fury Chromium core on Linux (Kaggle / VPS).
# Usage:
#   export GH_TOKEN="your_token"   # optional: to upload directly to GitHub release
#   ./tools/release/build-linux-core.sh [tag_name]
#
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TAG="${1:-v0.1.3}"
CHROMIUM_VERSION="$(cat "$HERE/core/CHROMIUM_VERSION")"

echo "=== 1. Checking dependencies ==="
sudo apt-get update -qq
sudo apt-get install -y -qq git python3 curl xz-utils build-essential lsb-release

echo "=== 2. Setting up depot_tools ==="
DEPOT_TOOLS="$HERE/core/depot_tools"
if [ ! -d "$DEPOT_TOOLS" ]; then
  git clone --depth 1 https://chromium.googlesource.com/chromium/tools/depot_tools.git "$DEPOT_TOOLS"
fi
export PATH="$DEPOT_TOOLS:$PATH"
export DEPOT_TOOLS_UPDATE=0

echo "=== 3. Fetching Chromium $CHROMIUM_VERSION ==="
# Allow skipping df check if running in container / Kaggle with ~70GB
export FORCE=1
"$HERE/core/build/fetch.sh" "$CHROMIUM_VERSION" || true

echo "=== 4. Applying Fury C++ patches ==="
"$HERE/core/build/apply.sh"

echo "=== 5. Building Chromium core for Linux x64 ==="
"$HERE/core/build/build.sh" linux-x64

echo "=== 6. Packaging Linux core archive ==="
"$HERE/tools/release/pack-core-linux.sh"

echo "=== 7. Uploading to GitHub Release (if token provided) ==="
if [ -n "${GH_TOKEN:-}" ]; then
  echo "Uploading core artifact to release $TAG..."
  gh release upload "$TAG" "$HERE/dist"/fury-core-*-linux-x64.tar.xz --clobber
  echo "Upload complete!"
fi

echo "=== Core build finished successfully! ==="
