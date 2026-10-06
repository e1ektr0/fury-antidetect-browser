# Native Windows worker verification

Verified on 2026-10-06 against the local, unpublished fork adaptation based on
`16321b5651dbec38a6cbedfa6ffec136615ed43d`.

## Environment

- Host: Microsoft Windows 11 Pro, version 10.0.28000, build 28000.
- Agent: adapted Fury 0.2.16, native Windows Rust build.
- Browser: actual upstream Fury Windows core release 0.2.16; CDP reports
  Chrome/155.0.8059.12.
- Archive SHA-256:
  `26859f982e9def0d6316840b1380e655dcdb9a3eca6ad56ad01c10de7fc14593`.
- Driver: Node.js 24.14.0, built-in fetch and WebSocket, no browser-driver package.

## Observed results

- `cargo check -p fury-agent --locked` passed.
- `cargo build -p fury-agent --release --locked` passed.
- Full agent suite: 173 tests passed, zero failures, after correcting the
  approved Windows version-resource test expectation.
- `rustfmt --check` passed for the newly authored management module; upstream
  files retain their surrounding formatting.
- Both Node.js scripts passed syntax checks; tracked-file diff checks passed.

- Profile create/get/update/delete and proxy create/update exercised through
  authenticated HTTP tests; established seeds retained and profile reads redact
  proxy secrets.
- Running-profile conflict and lifecycle-lock tests passed.
- Malformed/truncated/framing/timeout and bearer/Host/Origin tests passed.
- Occupied API port produced startup failure; disabled API left IPC working.
- Windows launch script rejected a missing executable and started the real
  agent with isolated persistent data and explicit core paths.
- Management example launched the real browser and reported a CDP endpoint.
- CDP Browser.close stopped test browsers cleanly, typically in 200-400 ms;
  the initial immediate-stop forced-kill fallback was resolved for CDP profiles.
- Persistence smoke passed: cookie and localStorage values survived profile
  stop/start. The fixture was delivered through exact-navigation CDP Fetch
  interception, preserving the relay's local-network refusal behavior.
- Separate write/read phases passed across an agent-process restart under the
  same account and data directory. Browsers were stopped before that restart.
- Missing-core smoke failed explicitly rather than reporting a skipped pass.
- Final release-agent smoke passed with the same actual Windows core.
- Test profiles were stopped and soft-deleted; recoverable browser data remains
  as designed. The write phase intentionally retained its profile until read.

## Reproducing a worker-restart check

With the agent running, execute:

```powershell
$Written = node tools/smoke-windows-worker.mjs --phase write --keep-profile `
  --token-file "C:\FuryWorker\api-token" | ConvertFrom-Json
# Restart the agent using the same account, data directory and core.
node tools/smoke-windows-worker.mjs --phase read --profile-id $Written.profileId `
  --token-file "C:\FuryWorker\api-token"
```

The default `--phase both` checks browser stop/start without restarting the
agent. The phase read command verifies existing values; it does not rewrite
them before checking.

## Verification boundaries

This is not a Windows Server, Timeweb VPS, RDP-disconnect, real portal login, or
proxy-network compatibility certification. Those require their respective live
environments. Fixture interception tests CDP and persistent storage, not site
scraping or access to local/private network resources. Worker cleanup has
controlled-process tests; the live browser tests exercised profile stop, not a
forced host reboot. API transport remains loopback-only and remote use needs
the documented tunnel or co-located worker.
