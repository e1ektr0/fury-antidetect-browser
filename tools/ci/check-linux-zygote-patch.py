#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Apply config startup patches to the actual pinned Chromium source file."""
import pathlib
import subprocess
import tempfile
import urllib.request

root = pathlib.Path(__file__).resolve().parents[2]
version = (root / "core/CHROMIUM_VERSION").read_text().strip()
filename = "content/app/content_main_runner_impl.cc"
url = f"https://raw.githubusercontent.com/chromium/chromium/{version}/{filename}"
with urllib.request.urlopen(url, timeout=30) as response:
    source = response.read()
with tempfile.TemporaryDirectory() as directory:
    work = pathlib.Path(directory)
    target = work / filename
    target.parent.mkdir(parents=True)
    target.write_bytes(source)
    for patch in ("0001-fp-config-plumbing.patch", "0002-linux-zygote-config.patch"):
        subprocess.run(["git", "apply", f"--include={filename}", str(root / "core/patches" / patch)], cwd=work, check=True)
    print(f"Linux zygote patch applies to Chromium {version}")
