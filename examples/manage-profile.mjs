// SPDX-License-Identifier: AGPL-3.0-or-later
// Node.js 24+: provision, update, launch, and stop a throwaway worker profile.
import { readFile } from 'node:fs/promises';
import { parseArgs } from 'node:util';

const { values } = parseArgs({ options: {
  base: { type: 'string', default: 'http://127.0.0.1:35000' },
  'token-file': { type: 'string' },
  'proxy-file': { type: 'string' },
  direct: { type: 'boolean', default: false },
} });
if (!values['token-file']) throw new Error('Pass --token-file <FURY_HOME/api-token>.');
if (!values['proxy-file'] && !values.direct) {
  throw new Error('Pass --proxy-file <proxy.json> or explicitly --direct for a local test.');
}
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
let profileId;
let proxyId;
try {
  if (values['proxy-file']) {
    const proxy = JSON.parse(await readFile(values['proxy-file'], 'utf8'));
    proxyId = (await api('POST', '/v1/proxies', proxy)).id;
    await api('PUT', `/v1/proxies/${proxyId}`, proxy);
  }
  const personas = await api('GET', '/v1/personas');
  const persona = personas.find(p => p.os.toLowerCase().includes('windows'));
  if (!persona) throw new Error('No Windows persona in the catalogue.');
  const input = { name: 'Worker API example', persona_id: persona.id,
    proxy_id: proxyId ?? null, allow_no_proxy: values.direct };
  profileId = (await api('POST', '/v1/profiles', input)).id;
  await api('PUT', `/v1/profiles/${profileId}`, { ...input, name: 'Updated worker API example' });
  const stored = await api('GET', `/v1/profiles/${profileId}`);
  console.log(JSON.stringify({ id: stored.id, name: stored.name, running: stored.running }));
  const launch = await api('POST', '/v1/profiles/start', { id: profileId, cdp: true });
  if (!launch.ws_endpoint) throw new Error('Browser started without an available CDP endpoint.');
  console.log(`Browser CDP (on the worker host): ${launch.ws_endpoint}`);
} finally {
  if (profileId) {
    await api('POST', '/v1/profiles/stop', { id: profileId });
    await api('DELETE', `/v1/profiles/${profileId}`);
  }
}
