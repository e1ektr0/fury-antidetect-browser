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

The imported system-browser fallback allows the agent to find ordinary Chromium,
Chrome, or Brave when no Fury core is installed. **Those browsers do not contain
Fury's fingerprint patches.** Automated integrations needing Fury must provision
the real core explicitly and verify its fingerprint before using it.

## Verification and current limits

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
