# Linux support in the e1ektr0 fork

This fork integrates the six Linux commits from `darakcheeff/main` through
`a5fc0dd1e7cdbdae07090bfef7a3571e81e04611`, onto the current Windows/macOS code.
The additions include a Linux x64 GN configuration, Chromium build and packaging
scripts, a Debian desktop package workflow, and resumable core builds.

## Build the agent and browser

On a Linux x64 build machine with Rust, Python 3.11+, Git, and sufficient disk:

```sh
cargo build --release --locked -p fury-agent
tools/release/build-linux-core.sh
target/release/fury-agent install-core dist/fury-core-*-linux-x64.tar.xz
```

The end-to-end script installs base build dependencies with `sudo`, fetches the
pinned `core/CHROMIUM_VERSION`, applies the tracked patches, builds `linux-x64`,
and packages the actual Fury core. Chromium's own build dependencies must also
be available; the `build-core.yml` workflow lists the supported runner setup.
The script does not suppress fetch failures.

Linux packages are named using `[workspace.package].version` in `Cargo.toml`.
Every core archive has a companion `.sha256` file:

```sh
cd dist
sha256sum -c fury-core-*-linux-x64.tar.xz.sha256
```

Run **Build Linux Core (Chromium)** or **Build Linux .deb Package** from GitHub
Actions. A blank release tag uses the current package version. Core compilation
may take multiple six-hour hosted-runner jobs: rerun the same commit to restore
its checkpoint. Checkpoints are commit-scoped so a changed patch or Chromium
version cannot accidentally reuse stale objects. Checkpoint cleanup happens only
after successful publication.

## Launch and automation

Use a Linux persona from `shared/personas/`, a working upstream HTTP/SOCKS proxy,
and an installed Fury core. Set `FURY_HOME` for an isolated state directory or
`FURY_CORE` to an explicit executable. The CLI supports `--profile-dir`,
`--timezone`, `--lang`, `--url`, and `--debug-port`.

Playwright connects to the running profile over CDP; use the existing browser
context as shown in `examples/playwright_example.py`. A visible Linux browser
requires an X server (for Docker, typically Xvfb) and Chromium runtime libraries.

The imported system-browser fallback is opt-in with `FURY_ALLOW_SYSTEM_BROWSER=1`.
It lets the agent find ordinary Chromium, Chrome, or Brave when no Fury core is
installed. **Those browsers do not contain
Fury's fingerprint patches.** Automated integrations needing Fury must provision
the real core explicitly and verify its fingerprint before using it.

## Verification and current limits

Linux renderers fork from the zygote and do not execute
`ContentMainRunnerImpl::Initialize()` again. Patch `0002-linux-zygote-config`
recovers their per-child fingerprint shared-memory region after descriptor
population in `RunZygote`, before delegates and Blink start. Without it a Windows
persona can report a Windows UA but Linux `navigator.platform` and the host's UTC
timezone. This was reproduced with the donor Chromium 153 core; bypassing the
zygote restored `Win32` and `America/Toronto`, confirming the missing startup path.

After building the corrected core, verify the normal zygote path with a real
browser (Node 22+, an X display or Xvfb, and the current compiled agent):

```sh
FURY_CORE=/path/to/Fury/chrome xvfb-run -a node tools/verify-linux.mjs
```

This checks native main-frame, Worker and iframe platform, timezone, and summer
UTC offset. It uses neither JS fingerprint shims nor CDP emulation. All three
must report `Win32`, `America/Toronto`, and `240`. It fails on the old uncorrected
zygote runtime. A Docker wrapper may add `--no-sandbox` where the container's
policy requires it, but must not add `--no-zygote` to acceptance verification.
The diagnostic no-zygote result validates the cause, not a newly compiled patch.

```sh
cargo test --workspace --exclude fury-desktop --all-targets
python3 tools/release/test_linux_packaging.py
```

The packaging test validates archive contents, executable modes, version naming,
checksums, and failure on missing inputs using temporary fixtures. It does not
claim to validate a Chromium build or a site's bot detection. The donor fork's
published `v0.1.3` Linux core is older than this fork's current Chromium pin;
build and verify a matching core before using the current release in production.

Upstream README statements that Linux is not a release target describe the
upstream project; this document describes this fork's added Linux build support.
