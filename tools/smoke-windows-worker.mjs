// SPDX-License-Identifier: AGPL-3.0-or-later
// Node.js 24, co-located with a running agent and the actual patched Fury core.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { parseArgs } from 'node:util';

const { values } = parseArgs({ options: {
  base: { type: 'string', default: 'http://127.0.0.1:35000' },
  'token-file': { type: 'string' },
  phase: { type: 'string', default: 'both' },
  'profile-id': { type: 'string' },
  'keep-profile': { type: 'boolean', default: false },
} });
if (!values['token-file']) throw new Error('Pass --token-file <FURY_HOME/api-token>.');
if (!['both', 'write', 'read'].includes(values.phase)) throw new Error('Phase must be both, write or read.');
if (values.phase === 'read' && !values['profile-id']) throw new Error('Read phase requires --profile-id.');
const token = (await readFile(values['token-file'], 'utf8')).trim();
async function api(method, path, input) {
  const response = await fetch(`${values.base}${path}`, {
    method, headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
    body: input === undefined ? undefined : JSON.stringify(input),
    signal: AbortSignal.timeout(120_000),
  });
  const result = await response.json();
  if (!response.ok || !result.ok) throw new Error(`${response.status}: ${result.message}`);
  return result.data;
}
async function cdp(endpoint) {
  if (!/^ws:\/\/(127\.0\.0\.1|localhost):\d+\//.test(endpoint ?? '')) {
    throw new Error('Expected a co-located loopback CDP endpoint.');
  }
  const socket = new WebSocket(endpoint);
  const pending = new Map();
  let nextId = 0;
  let eventHandler;
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { socket.close(); reject(new Error('CDP connection timeout')); }, 10_000);
    socket.addEventListener('open', () => { clearTimeout(timer); resolve(); }, { once: true });
    socket.addEventListener('error', () => { clearTimeout(timer); reject(new Error('CDP connection failed')); }, { once: true });
  });
  socket.addEventListener('message', event => {
    const message = JSON.parse(event.data);
    if (message.method) {
      eventHandler?.(message);
      return;
    }
    const request = pending.get(message.id);
    if (!request) return;
    pending.delete(message.id);
    clearTimeout(request.timer);
    if (message.error) request.reject(new Error(JSON.stringify(message.error)));
    else request.resolve(message.result);
  });
  socket.addEventListener('close', () => {
    for (const request of pending.values()) { clearTimeout(request.timer); request.reject(new Error('CDP closed')); }
    pending.clear();
  });
  return {
    call(method, params = {}, sessionId) {
      const id = ++nextId;
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 10_000);
        pending.set(id, { resolve, reject, timer });
        socket.send(JSON.stringify({ id, method, params, sessionId }));
      });
    },
    close() { socket.close(); },
    onEvent(handler) { eventHandler = handler; },
  };
}

const status = await api('GET', '/v1/status');
if (!status.core || status.core_outdated) throw new Error('A usable real Fury core is required; smoke test is not skipped.');
const fixture = createServer((_req, res) => {
  res.writeHead(200, { 'Content-Type': 'text/html', 'Cache-Control': 'no-store' });
  res.end('<!doctype html><title>Fury worker persistence fixture</title><p>Local test only</p>');
});
fixture.listen(0, '127.0.0.1');
await once(fixture, 'listening');
const fixtureUrl = `http://127.0.0.1:${fixture.address().port}`;
// A stable test origin also permits write/read phases across worker restarts.
// The exact test navigation is intercepted and does not require DNS.
const origin = 'http://fury-worker-fixture.test';
// The driver reads the local fixture; the browser receives it via a scoped
// Fetch interception. Fury's relay intentionally refuses local-network URLs.
let fixtureHtml;
let profileId = values['profile-id'];
let connection;
let product;
try {
  fixtureHtml = await (await fetch(fixtureUrl)).text();
  const personas = await api('GET', '/v1/personas');
  const persona = personas.find(p => p.os.toLowerCase().includes('windows'));
  if (!persona) throw new Error('Windows persona required.');
  profileId ??= (await api('POST', '/v1/profiles', {
    name: 'Worker persistence smoke', persona_id: persona.id,
    allow_no_proxy: true, start_urls: ['about:blank'],
  })).id;
  for (const phase of values.phase === 'both' ? ['write', 'read'] : [values.phase]) {
    const launch = await api('POST', '/v1/profiles/start', { id: profileId, cdp: true });
    assert.equal((await api('GET', `/v1/profiles/${profileId}`)).running, true);
    connection = await cdp(launch.ws_endpoint);
    product = (await connection.call('Browser.getVersion')).product;
    const target = await connection.call('Target.createTarget', { url: 'about:blank' });
    const { sessionId } = await connection.call('Target.attachToTarget', { targetId: target.targetId, flatten: true });
    let interceptionError;
    let fulfilled = 0;
    const client = connection;
    connection.onEvent(event => {
      if (event.method !== 'Fetch.requestPaused' || event.sessionId !== sessionId) return;
      client.call('Fetch.fulfillRequest', {
        requestId: event.params.requestId, responseCode: 200,
        responseHeaders: [{ name: 'Content-Type', value: 'text/html' }, { name: 'Cache-Control', value: 'no-store' }],
        body: Buffer.from(fixtureHtml).toString('base64'),
      }, sessionId).then(() => { fulfilled++; }, error => { interceptionError = error; });
    });
    await connection.call('Fetch.enable', { patterns: [{ urlPattern: `${origin}/`, requestStage: 'Request' }] }, sessionId);
    await connection.call('Page.navigate', { url: `${origin}/` }, sessionId);
    let ready = false;
    for (let i = 0; i < 50; i++) {
      if (interceptionError) throw interceptionError;
      const result = await connection.call('Runtime.evaluate', {
        expression: `location.origin === ${JSON.stringify(origin)} && document.readyState === 'complete'`, returnByValue: true,
      }, sessionId);
      if (result.result.value === true) { ready = true; break; }
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    assert.ok(fulfilled > 0, 'the exact fixture navigation must be intercepted');
    if (!ready) {
      const diagnostic = await connection.call('Runtime.evaluate', {
        expression: '({href:location.href, ready:document.readyState, text:document.body?.innerText})', returnByValue: true,
      }, sessionId);
      throw new Error(`Local fixture did not load: ${JSON.stringify(diagnostic)}`);
    }
    if (phase === 'write') {
      const written = await connection.call('Runtime.evaluate', {
        expression: "document.cookie='worker_session=retained; Max-Age=3600; Path=/'; localStorage.setItem('worker_session','retained'); true",
        returnByValue: true,
      }, sessionId);
      assert.equal(written.result.value, true);
    }
    const observed = await connection.call('Runtime.evaluate', {
      expression: "({cookie:document.cookie, storage:localStorage.getItem('worker_session')})", returnByValue: true,
    }, sessionId);
    assert.match(observed.result.value.cookie, /worker_session=retained/);
    assert.equal(observed.result.value.storage, 'retained');
    connection.close();
    connection = undefined;
    await api('POST', '/v1/profiles/stop', { id: profileId });
    assert.equal((await api('GET', `/v1/profiles/${profileId}`)).running, false);
  }
  console.log(JSON.stringify({ passed: true, phase: values.phase, profileId, agent: status.version, browser: product,
    platform: process.platform, core: status.core, checks: ['CDP', 'cookie persistence', 'localStorage persistence', 'start/stop status'] }));
} finally {
  connection?.close();
  try {
    if (profileId) {
      await api('POST', '/v1/profiles/stop', { id: profileId });
      if (!values['keep-profile']) await api('DELETE', `/v1/profiles/${profileId}`);
    }
  } finally {
    fixture.closeAllConnections();
    await new Promise(resolve => fixture.close(resolve));
  }
}
