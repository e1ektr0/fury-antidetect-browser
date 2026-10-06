# Native Windows browser worker

Run the actual Fury browser and its automation driver on Windows. Linux can
host the application, web UI, and PostgreSQL. The existing Docker Compose team
server stores team metadata/bundles; it does not execute browsers.

```text
Linux application + PostgreSQL jobs
                 |
         future portal job runner
                 |
Windows: portal driver --> local Fury API --> Fury browser
```

The PostgreSQL job runner and Pikabu integration are separate work. This setup
provides browser/profile management, not a complete Avatar deployment.

## Build and start

Use a Windows account with an interactive desktop, a current Rust/MSVC build
toolchain, and the matching real Windows Fury core from upstream releases.
Build the agent from the adapted fork:

```powershell
cargo build -p fury-agent --release --locked
pwsh -File tools/start-windows-worker.ps1 `
  -AgentPath "C:\Fury\fury-agent.exe" `
  -CorePath "C:\Fury\Core\chrome.exe" `
  -DataDirectory "C:\FuryWorker" -ApiPort 35000
```

The script validates both executables, creates the persistent data directory,
sets `FURY_HOME`, `FURY_CORE`, and `FURY_API_PORT`, and propagates the agent exit
code. An occupied API port is a startup failure, not a worker that silently
continues without its API. The default agent command remains compatible:
without a valid positive `FURY_API_PORT`, no automation HTTP port is bound.

The API bearer token lives at `C:\FuryWorker\api-token`. Read it from that file;
do not put it into source control. The database, profile directories, and log
files remain under the configured directory. Keep the same Windows account:
the OS keyring is separate from the directory, so directory backups alone are
not proof that sealed secrets can be restored under another account.

## API behavior

See [automation examples](../examples/README.md) for profile/proxy management,
payloads, and the Node.js management example. Management uses the existing
persona checks; arbitrary fingerprint blobs and caller-selected seeds are not
accepted. A missing resource returns 404 and a running-profile mutation 409.
Existing start/stop envelopes and legacy error behavior remain available.

All requests need a bearer token and a loopback Host. Requests carrying Origin
are refused even with a token. The panel calls application APIs, not Fury from
cross-origin browser JavaScript.

Headers are limited to 16 KiB, bodies to 1 MiB, and request receipt to five
seconds. Malformed JSON, conflicting/duplicate sensitive headers, unsupported
transfer encoding, and incomplete framing return 400; receipt timeout returns
408. Responses close the connection. Execution of a valid browser launch is
separate from the request-receipt timeout.

## Shutdown

Ctrl+C or Ctrl+Break requests cleanup. The agent also handles supported console
close/logoff/shutdown notifications, but Windows imposes time limits on those;
prefer stopping profiles explicitly before logging off or rebooting. A forced
`Stop-Process`/TerminateProcess is not graceful and can lose recent session data.

For explicitly CDP-enabled profiles, stop requests `Browser.close` before the
native window/signal fallback. Profiles without CDP retain native shutdown.
Browsers that refuse to exit are bounded by the existing ten-second fallback,
which is logged as a forced kill rather than claimed as a clean session flush.

## Interactive authentication and restart

Open a profile under the worker account and complete the portal login in its
browser. The portal adapter will later verify the authenticated identity; a
running browser alone does not prove Pikabu authentication.

For unattended restart after login, create a Scheduled Task **at logon** for
that account, using “Run only when user is logged on”, with `pwsh.exe` as the
action and the launch command above as arguments. Configure restart on failure
and disable the task's default execution-time limit. This is not a Session 0
Windows service installation. Test RDP disconnect separately on the chosen VPS;
disconnecting and logging off are different operations.

## Remote administration

Keep the API and CDP loopback-only. With Windows OpenSSH Server configured, a
Linux operator can forward the API:

```sh
ssh -N -L 35000:127.0.0.1:35000 worker@windows-host
```

Use `http://127.0.0.1:35000` with the worker's bearer token. A returned
`ws://127.0.0.1:<debug-port>/...` refers to **the Windows host**. Prefer a driver
on that host. A remote driver needs an additional tunnel for the returned port:

```sh
ssh -N -L 51835:127.0.0.1:51835 worker@windows-host
```

Here 51835 is an example; use the actual response's debug port and forward the
same local port to use the returned endpoint unchanged. Neither an SSH tunnel
nor the API token turns a raw public CDP port into an authenticated API.

## Smoke verification

With a running agent and Node.js 24, run:

```powershell
node tools/smoke-windows-worker.mjs `
  --base http://127.0.0.1:35000 --token-file "C:\FuryWorker\api-token"
```

The driver reads a local controlled HTTP site and serves the exact test
navigation through CDP Fetch interception. The relay's local-network protection
remains enabled; this checks storage persistence, not local-network access.
The test explicitly permits a proxy-free throwaway profile, connects to the
real core through CDP, and checks cookie and local-storage persistence across
stop/start. It stops and soft-deletes the test
profile. The script is for co-located execution and fails if no usable core or
CDP connection exists. It does not substitute stock Chromium or label a missing
browser as a passing test.

Agent/API tests and the real browser test are separate. A passing Windows 11
test does not establish Windows Server readiness, an RDP-disconnect result, or
the number of concurrent browsers the VPS can sustain.

See [verification results](windows-worker-verification.md) for the measured
Windows 11 environment and the separate worker-restart procedure. SSH forwarding
examples were syntax-checked locally; no Windows VPS or remote SSH connection
was provisioned during this verification.
