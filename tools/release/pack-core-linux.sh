#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# Pack the Linux x64 core into the shape install-core expects.
#
# Usage:
#     tools/release/pack-core-linux.sh
#
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
version="$(cat "$here/core/CHROMIUM_VERSION" 2>/dev/null || echo "0.1.3")"
out="${OUT_DIR:-$here/core/src/out/linux-x64}"
dist="${DIST:-$here/dist}"

if [ ! -d "$out" ]; then
  echo "!! Output directory not found: $out" >&2
  exit 1
fi
cd "$out"

files=(
  chrome chrome_crashpad_handler
  libEGL.so libGLESv2.so libvulkan.so.1
  chrome_100_percent.pak chrome_200_percent.pak resources.pak
  icudtl.dat v8_context_snapshot.bin
)

# Optional swiftshader / extra libs if built
for opt in libvk_swiftshader.so vk_swiftshader_icd.json chrome-sandbox; do
  if [ -f "$opt" ]; then
    files+=("$opt")
  fi
done

dirs=(locales resources MEIPreload)

missing=0
for f in "${files[@]}"; do
  [ -f "$f" ] || { echo "!! missing: $f" >&2; missing=1; }
done
[ "$missing" = 0 ] || exit 1

rm -rf "$here/dist-core" && mkdir -p "$here/dist-core/Fury"
cp -a "${files[@]}" "$here/dist-core/Fury/"
for d in "${dirs[@]}"; do [ -d "$d" ] && cp -r "$d" "$here/dist-core/Fury/"; done

echo "== staged linux core"
du -sh "$here/dist-core/Fury" | cut -f1

echo "== packing"
mkdir -p "$dist"
pkg_version="$(cargo metadata --format-version 1 --no-deps | grep -o '"version":"[^"]*"' | head -1 | cut -d'"' -f4 || echo "0.1.3")"
out_name="fury-core-$pkg_version-linux-x64.tar.xz"
tar -cJf "$dist/$out_name" -C "$here/dist-core" Fury
ls -la "$dist/$out_name" | awk '{printf "   %.0f MB\n", $5/1000000}'
echo "Packaged into: $dist/$out_name"
