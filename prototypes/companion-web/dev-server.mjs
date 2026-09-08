// Isolated, loopback-only conformance fixture. This is NOT a gateway or Engine.
import { createServer } from 'node:http';
import { createHash, randomBytes, randomUUID } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

const STATIC = new Map([['/', ['index.html', 'text/html']], ...['index.html', 'app.css', 'app.js', 'client.js', 'model.js', 'sw.js', 'icon.svg', 'manifest.webmanifest'].map(name => [`/${name}`, [name, name.endsWith('.js') ? 'text/javascript' : name.endsWith('.css') ? 'text/css' : name.endsWith('.svg') ? 'image/svg+xml' : name.endsWith('.webmanifest') ? 'application/manifest+json' : 'text/html']])]);
const BODY_LIMIT = 16 * 1024;
const json = (response, status, body) => { response.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff', 'Referrer-Policy': 'no-referrer' }); response.end(JSON.stringify(body)); };
async function body(request) { let text = ''; for await (const chunk of request) { text += chunk; if (Buffer.byteLength(text) > BODY_LIMIT) throw new Error('oversized'); } return JSON.parse(text); }
const copy = value => structuredClone(value);
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const fail = (response, code, status = 409) => json(response, status, { code });

export async function createFixture({ port = 0 } = {}) {
  let engineEpoch = 'engine_fixture_1', streamId = 'events_fixture_1', sequence = 0, mode = 'online';
  const serverId = 'server_fixture_74';
  const enrollments = new Map(), devices = new Map(), sockets = new Set(), mutations = new Map();
  const stats = { prompts: 0, actions: 0, acquisitions: 0, requests: 0, subscriptions: 0, pairings: 0, replayCursors: [] };
  const now = Date.now();
  const projects = [{ id: 'project_local', name: 'Zeus', root: '/work/zeus', host: null }, { id: 'project_remote', name: 'API service', root: '/srv/api', host: 'build-box · SSH' }];
  const sessions = projects.map((project, index) => ({ id: index ? 'session_remote' : 'session_local', project_id: project.id, kind: index ? 'claude' : 'codex', title: index ? 'Review the API migration' : 'Ship the Companion prototype', cwd: project.root, host: project.host, status: index ? 'question' : 'working', created_at_ms: now - 3600000, updated_at_ms: now, archived: false, hibernated: false, revision: `revision_${index}_1` }));
  const screens = new Map(sessions.map((session, index) => [session.id, { session_id: session.id, incarnation: `incarnation_${index}_1`, screen_sequence: 1, text: index ? 'Migration ready for review.\nShould I apply the compatibility fix?' : 'Companion prototype\nChecking the mobile workflow…\nReady for your next prompt.', cols: 80, rows: 24, cursor_row: 2, cursor_col: 0, control: { epoch: { incarnation: `incarnation_${index}_1`, generation: 1 }, owner: { id: 'desktop_fixture', label: 'Zeus desktop', role: 'desktop' }, command_seq: 0 }, exited: false, truncated: false }]));
  let base;
  const issuePairing = (scopes = ['read', 'interact', 'lifecycle']) => {
    const code = randomBytes(32).toString('hex'), expires_at_ms = Date.now() + 120000;
    enrollments.set(code, { expires_at_ms, scopes });
    return { origin: base, server_id: serverId, code, expires_at_ms };
  };
  const emit = (kind = 'changed') => {
    const event = { cursor: { stream_id: streamId, sequence: ++sequence }, kind };
    for (const peer of sockets) if (peer.device && !peer.device.revoked) peer.send(event);
  };
  const setMode = value => {
    mode = value;
    if (value === 'revoked') { for (const device of devices.values()) device.revoked = true; for (const peer of sockets) { peer.send({ code: 'revoked' }); peer.socket.end(); } }
    if (value === 'offline') for (const peer of sockets) peer.socket.destroy();
    if (value === 'desktop') { for (const screen of screens.values()) { screen.control.epoch.generation++; screen.control.owner = { id: 'desktop_fixture', label: 'Zeus desktop', role: 'desktop' }; } emit(); }
    if (value === 'restart') { engineEpoch = 'engine_' + randomUUID(); streamId = 'stream_' + randomUUID(); sequence = 0; for (const screen of screens.values()) { screen.incarnation = randomUUID(); screen.control.epoch = { incarnation: screen.incarnation, generation: 1 }; screen.control.owner = null; } emit('resync_required'); }
    if (value === 'gap') { sequence += 10; emit(); }
  };
  const authenticate = request => { const token = request.headers.authorization?.replace(/^Bearer /u, ''); const device = devices.get(token); return device && !device.revoked && device.expires_at_ms > Date.now() ? device : null; };
  const isLocal = request => request.headers.host === new URL(base).host && (!request.headers.origin || request.headers.origin === base) && !['cross-site', 'same-site'].includes(request.headers['sec-fetch-site']);
  const server = createServer(async (request, response) => {
    stats.requests++;
    try {
      if (!isLocal(request)) return fail(response, 'forbidden', 403);
      const url = new URL(request.url, base), path = url.pathname;
      if (request.method === 'GET' && path === '/fixture') {
        const payload = issuePairing();
        response.writeHead(200, { 'Content-Type': 'text/html', 'Cache-Control': 'no-store', 'Content-Security-Policy': "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'" });
        response.end(`<!doctype html><html lang="en"><meta name="viewport" content="width=device-width"><title>Zeus local fixture</title><body style="font:18px system-ui;max-width:700px;margin:2rem;padding:1rem"><h1>Local fixture · synthetic data only</h1><p>Pairing expires in two minutes. This page is fixture tooling, never shipped with the PWA.</p><label for="fixture-payload">Pairing payload</label><textarea id="fixture-payload" rows="8" style="width:100%">${JSON.stringify(payload)}</textarea><p><a href="/">Open Companion</a></p><p>Use the test suite for deterministic revocation, replay, reconnect, and failure scenarios.</p></body></html>`); return;
      }
      if (request.method === 'GET' && STATIC.has(path) && !url.search) {
        const [file, type] = STATIC.get(path); const content = await readFile(new URL(`./public/${file}`, import.meta.url));
        response.writeHead(200, { 'Content-Type': type, 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff', 'Referrer-Policy': 'no-referrer', 'Permissions-Policy': 'camera=(), microphone=(), geolocation=()', 'Content-Security-Policy': "frame-ancestors 'none'" }); response.end(content); return;
      }
      if (request.method === 'POST' && path === '/v1/pair') {
        const data = await body(request);
        if (data.api_major !== 1) return fail(response, 'incompatible_protocol');
        if (data.expected_server_id !== serverId) return fail(response, 'server_identity_mismatch');
        const enrollment = enrollments.get(data.code);
        if (!enrollment || enrollment.expires_at_ms <= Date.now()) return fail(response, 'pairing_expired', 401);
        if (typeof data.device_name !== 'string' || !data.device_name.trim() || Buffer.byteLength(data.device_name) > 80) return fail(response, 'invalid_device');
        enrollments.delete(data.code);
        stats.pairings++;
        const token = randomBytes(32).toString('hex'), device = { id: randomBytes(16).toString('hex'), name: data.device_name, scopes: enrollment.scopes, expires_at_ms: Date.now() + 3600000, revoked: false }; devices.set(token, device);
        return json(response, 200, { server_id: serverId, device_id: device.id, token, scopes: device.scopes, expires_at_ms: device.expires_at_ms });
      }
      const device = authenticate(request);
      if (!device) return fail(response, 'unauthorized', 401);
      if (mode === 'offline') { request.socket.destroy(); return; }
      if (request.method === 'GET' && path === '/v1/hello') return json(response, 200, { server_id: serverId, api_major: mode === 'incompatible' ? 9 : 1, api_minor: 0, capabilities: ['projects', 'sessions', 'screen', 'events', 'rename', 'lifecycle', 'control_lease', 'send_text'], engine_epoch: engineEpoch, max_body_bytes: BODY_LIMIT, max_response_bytes: 256 * 1024 });
      if (request.method === 'GET' && ['/v1/projects', '/v1/sessions'].includes(path)) {
        const offset = Number(url.searchParams.get('offset') ?? 0), limit = Math.min(Number(url.searchParams.get('limit') ?? 64), 64), list = path.endsWith('projects') ? projects : sessions;
        if (!Number.isInteger(offset) || offset < 0 || !Number.isInteger(limit) || limit < 1) return fail(response, 'invalid_page');
        return json(response, 200, { items: list.slice(offset, offset + limit), next_offset: offset + limit < list.length ? offset + limit : null, engine_epoch: engineEpoch });
      }
      if (request.method === 'POST' && path === `/v1/devices/${device.id}/revoke`) { await body(request); device.revoked = true; for (const peer of sockets) if (peer.device === device) { peer.send({ code: 'revoked' }); peer.socket.end(); } return json(response, 200, { revoked: true }); }
      const match = /^\/v1\/sessions\/([A-Za-z0-9_-]+)(?:\/(screen|actions|control\/acquire|control\/release|text))?$/u.exec(path);
      const session = match && sessions.find(item => item.id === match[1]);
      if (!session) return fail(response, 'not_found', 404);
      const screen = screens.get(session.id), operation = match[2];
      if (request.method === 'GET' && !operation) return json(response, 200, { session, engine_epoch: engineEpoch });
      if (request.method === 'GET' && operation === 'screen') return json(response, 200, mode === 'oversized' ? { ...screen, text: 'x'.repeat(300000) } : screen);
      if (request.method !== 'POST') return fail(response, 'not_found', 404);
      const data = await body(request);
      if (['control/acquire', 'control/release', 'text'].includes(operation)) {
        if (!device.scopes.includes('interact')) return fail(response, 'forbidden', 403);
        if (!same(data.expected, screen.control.epoch)) return fail(response, 'stale_controller');
        if (operation === 'control/acquire') { if (data.takeover !== true) return fail(response, 'confirmation_required'); screen.control.owner = { id: device.id, label: device.name, role: 'mobile' }; screen.control.epoch.generation++; stats.acquisitions++; }
        else {
          if (screen.control.owner?.id !== device.id) return fail(response, 'stale_controller');
          if (operation === 'control/release') { screen.control.owner = null; screen.control.epoch.generation++; }
          else {
            if (data.command_seq !== screen.control.command_seq + 1) return fail(response, 'duplicate_command');
            if (typeof data.text !== 'string' || !data.text.trim() || Buffer.byteLength(data.text) > 8192 || data.submit !== true) return fail(response, 'invalid_prompt');
            screen.control.command_seq = data.command_seq; stats.prompts++; screen.screen_sequence++; screen.text = 'Prompt received.\nWorking on the requested change…'; session.status = 'working'; session.updated_at_ms = Date.now();
          }
        }
        emit();
        if (mode === 'drop_mutation') { response.destroy(); return; }
        return json(response, 200, screen.control);
      }
      if (operation === 'actions') {
        if (!device.scopes.includes('lifecycle')) return fail(response, 'forbidden', 403);
        if (data.engine_epoch !== engineEpoch || data.expected_revision !== session.revision) return fail(response, 'stale_revision');
        if (mutations.has(data.mutation_id)) return fail(response, 'duplicate_mutation');
        const kind = data.action?.kind;
        if (!['rename', 'archive', 'wake', 'hibernate', 'terminate'].includes(kind)) return fail(response, 'forbidden', 403);
        if (kind !== 'rename' && (data.action.confirmed !== true || !same(data.expected_control, screen.control.epoch))) return fail(response, 'confirmation_required');
        if (kind === 'rename') { if (typeof data.action.title !== 'string' || !data.action.title.trim()) return fail(response, 'invalid_title'); session.title = data.action.title; }
        if (kind === 'archive') session.archived = true;
        if (kind === 'wake') session.hibernated = false;
        if (kind === 'hibernate') session.hibernated = true;
        if (kind === 'terminate') { session.status = 'done'; screen.exited = true; }
        session.revision = randomUUID(); stats.actions++; mutations.set(data.mutation_id, true); emit();
        if (mode === 'drop_mutation') { response.destroy(); return; }
        return json(response, 200, { mutation_id: data.mutation_id, applied: true });
      }
      return fail(response, 'not_found', 404);
    } catch { if (!response.headersSent) fail(response, 'invalid_request', 400); else response.end(); }
  });
  // Tiny RFC6455 fixture seam: one bounded, masked, unfragmented subscribe frame.
  // Deliberately not a reusable production WebSocket implementation.
  server.on('upgrade', (request, socket, head) => {
    if (!isLocal(request) || request.url !== '/v1/events' || mode === 'offline' || request.headers['sec-websocket-version'] !== '13' || typeof request.headers['sec-websocket-key'] !== 'string') { socket.destroy(); return; }
    const accept = createHash('sha1').update(request.headers['sec-websocket-key'] + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64');
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
    const peer = { socket, device: null, send(value) { const data = Buffer.from(JSON.stringify(value)); if (socket.writableLength > 65536 || data.length > 65535) { socket.destroy(); return; } const header = data.length < 126 ? Buffer.from([0x81, data.length]) : Buffer.from([0x81, 126, data.length >> 8, data.length & 255]); socket.write(Buffer.concat([header, data])); } };
    sockets.add(peer); let buffered = Buffer.alloc(0);
    const timeout = setTimeout(() => socket.destroy(), 5000);
    socket.on('close', () => { clearTimeout(timeout); sockets.delete(peer); }); socket.on('error', () => {});
    const receive = chunk => {
      buffered = Buffer.concat([buffered, chunk]);
      if (buffered.length > BODY_LIMIT) { socket.destroy(); return; }
      if (buffered.length < 2) return;
      if ((buffered[0] & 15) === 8) { socket.end(Buffer.from([0x88, 0])); return; }
      if (peer.device || buffered[0] !== 0x81 || !(buffered[1] & 128)) { socket.destroy(); return; }
      let size = buffered[1] & 127, start = 2;
      if (size === 127) { socket.destroy(); return; }
      if (size === 126) { if (buffered.length < 4) return; size = buffered.readUInt16BE(2); start = 4; }
      if (size > BODY_LIMIT - 8) { socket.destroy(); return; }
      if (buffered.length < start + 4 + size) return;
      const mask = buffered.subarray(start, start + 4), data = Buffer.from(buffered.subarray(start + 4, start + 4 + size));
      for (let index = 0; index < data.length; index++) data[index] ^= mask[index % 4];
      try {
        const value = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(data)), device = devices.get(value.token);
        if (!device || device.revoked || device.expires_at_ms <= Date.now()) { peer.send({ code: 'revoked' }); socket.end(); return; }
        if (value.api_major !== 1) { peer.send({ code: 'incompatible_protocol' }); socket.end(); return; }
        peer.device = device; clearTimeout(timeout); stats.subscriptions++; stats.replayCursors.push(copy(value.cursor));
        peer.send({ cursor: { stream_id: streamId, sequence: ++sequence }, kind: value.cursor && (value.cursor.stream_id !== streamId || value.cursor.sequence < sequence - 128) ? 'resync_required' : 'changed' });
        buffered = Buffer.alloc(0);
      } catch { socket.destroy(); }
    };
    socket.on('data', receive); if (head.length) receive(head);
  });
  server.requestTimeout = 10000; server.headersTimeout = 10000; server.maxRequestsPerSocket = 1000;
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(port, '127.0.0.1', resolve); });
  base = `http://127.0.0.1:${server.address().port}`;
  return { origin: base, issuePairing, setMode, emit, stats, setScreenText(id, text) { const screen = screens.get(id); screen.text = text; screen.screen_sequence++; emit(); }, close: async () => { for (const peer of sockets) peer.socket.destroy(); server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); } };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const fixture = await createFixture({ port: Number(process.env.COMPANION_FIXTURE_PORT ?? 4174) });
  process.stdout.write(`Zeus Companion fixture: ${fixture.origin}\nPairing screen: ${fixture.origin}/fixture\nSynthetic data only; no Engine connection.\n`);
  process.on('SIGINT', () => { void fixture.close().then(() => process.exit(0)); });
  process.on('SIGTERM', () => { void fixture.close().then(() => process.exit(0)); });
}
