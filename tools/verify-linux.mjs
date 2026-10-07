// SPDX-License-Identifier: AGPL-3.0-or-later
// Real Linux core regression check: no JS shims or CDP emulation overrides.
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createServer } from 'node:net';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const agent = process.env.FURY_AGENT || path.join(root, 'target/release/fury-agent');
const core = process.env.FURY_CORE;
if (process.platform !== 'linux' || !core) throw new Error('Run on Linux with FURY_CORE set to a patched core executable');
const profile = await mkdtemp(path.join(tmpdir(), 'fury-linux-zygote-'));
const reserve = createServer();
await new Promise(resolve => reserve.listen(0, '127.0.0.1', resolve));
const port = reserve.address().port;
await new Promise(resolve => reserve.close(resolve));
const child = spawn(agent, [
  'launch', process.env.FURY_PERSONA || path.join(root, 'shared/personas/windows-11-rtx4060-1920x1080.json'),
  '--core', core, '--proxy', 'http://127.0.0.1:1',
  '--profile-dir', path.join(profile, 'browser'), '--debug-port', String(port),
  '--timezone', 'America/Toronto', '--lang', 'en-US,en', '--url', 'about:blank',
], { detached: true, env: { ...process.env, FURY_HOME: path.join(profile, 'home') }, stdio: 'ignore' });
let socket;
let counter = 0;
const pending = new Map();
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
function send(method, params = {}) {
  const id = ++counter;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { pending.delete(id); reject(new Error('CDP timeout')); }, 10000);
    pending.set(id, message => { clearTimeout(timer); message.error ? reject(new Error(message.error.message)) : resolve(message.result); });
    socket.send(JSON.stringify({ id, method, params }));
  });
}
try {
  let version;
  for (let i = 0; i < 100; i++) {
    if (child.exitCode !== null) throw new Error('Fury exited before CDP became available');
    try { version = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json(); break; } catch {}
    await pause(200);
  }
  if (!version) throw new Error('No Fury CDP endpoint');
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  const target = targets.find(t => t.type === 'page');
  socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  socket.onmessage = event => { const message = JSON.parse(event.data); pending.get(message.id)?.(message); pending.delete(message.id); };
  const expression = `(async()=>{
    const snapshot=()=>({platform:navigator.platform,timeZone:Intl.DateTimeFormat().resolvedOptions().timeZone,offset:new Date('2026-07-01T12:00:00Z').getTimezoneOffset()});
    const main=snapshot();
    const workerSource='postMessage(('+snapshot.toString()+')())';
    const url=URL.createObjectURL(new Blob([workerSource],{type:'text/javascript'}));
    const worker=new Worker(url);
    const workerResult=await new Promise((resolve,reject)=>{worker.onmessage=e=>resolve(e.data);worker.onerror=reject;});
    worker.terminate();URL.revokeObjectURL(url);
    const iframe=document.createElement('iframe');iframe.srcdoc='<html></html>';
    await new Promise(resolve=>{iframe.onload=resolve;document.body.append(iframe);});
    const win=iframe.contentWindow;
    const frame={platform:win.navigator.platform,timeZone:new win.Intl.DateTimeFormat().resolvedOptions().timeZone,offset:new win.Date('2026-07-01T12:00:00Z').getTimezoneOffset()};
    iframe.remove();return {main,worker:workerResult,iframe:frame};
  })()`;
  const response = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
  if (response.exceptionDetails) throw new Error('Linux context verification failed');
  const contexts = response.result.value;
  const passed = Object.values(contexts).every(value => value.platform === 'Win32' && value.timeZone === 'America/Toronto' && value.offset === 240);
  console.log(JSON.stringify({ test: 'linux-zygote-fingerprint', browser: version.Browser, passed, contexts }, null, 2));
  if (!passed) process.exitCode = 1;
} finally {
  if (socket?.readyState === WebSocket.OPEN) { try { await send('Browser.close'); } catch {} socket.close(); }
  if (child.exitCode === null) { try { process.kill(-child.pid, 'SIGTERM'); } catch {} }
  await rm(profile, { recursive: true, force: true });
}
