import test from 'node:test';
import assert from 'node:assert/strict';
import { createFixture } from '../dev-server.mjs';
import { CompanionClient, boundedJSON, validateHello, validateScreen } from '../public/client.js';
import { EventCursor, LIMITS, confirmation, errorMessage, origin, pairingPayload, promptText, terminalText } from '../public/model.js';

const code = expected => error => error.code === expected;
async function setup(t, options = {}) {
  const fixture = await createFixture();
  const client = new CompanionClient({ currentOrigin: fixture.origin, ...options });
  t.after(async () => { client.forget(); await fixture.close(); });
  await client.pair(fixture.issuePairing(), 'Fixture phone');
  return { fixture, client };
}
const waitFor = async predicate => { const deadline = Date.now() + 3000; while (!predicate()) { assert.ok(Date.now() < deadline, 'event arrived before deadline'); await new Promise(resolve => setTimeout(resolve, 10)); } };

test('pairing binds to same HTTPS origin and expected identity with a short deadline', () => {
  const now = 1000000, value = { origin: 'https://zeus.example', server_id: 'server_74', code: 'fixture', expires_at_ms: now + 120000 };
  assert.deepEqual(pairingPayload(JSON.stringify(value), value.origin, now), value);
  assert.throws(() => origin('http://zeus.example', 'http://zeus.example'), code('https_required'));
  assert.throws(() => origin('https://evil.example', value.origin), code('same_origin_required'));
  for (const originValue of ['https://user:pass@zeus.example', 'https://zeus.example/?token=bad', 'https://zeus.example/#bad', 'https://zeus.example/path']) assert.throws(() => origin(originValue, value.origin), code('invalid_origin'));
  for (const expiry of [now - 1, now + 600001]) assert.throws(() => pairingPayload(JSON.stringify({ ...value, expires_at_ms: expiry }), value.origin, now), code('pairing_expired'));
});

test('text is bounded by UTF-8 bytes and terminal controls are inert', () => {
  assert.throws(() => promptText(' '), code('empty_prompt'));
  assert.throws(() => promptText('\u001b[2J'), code('invalid_prompt'));
  assert.throws(() => promptText('界'.repeat(3000)), code('incompatible_response'));
  assert.equal(terminalText('<script>alert(1)</script>'), '<script>alert(1)</script>');
  assert.equal(terminalText('safe\u202eevil\u001b'), 'safe�evil�');
  assert.equal(promptText('Explain the fix\nand add coverage.'), 'Explain the fix\nand add coverage.');
  assert.ok(!errorMessage('secret prompt from server').includes('secret'));
});

test('bounded decoding rejects declared and streaming oversized data, malformed JSON and UTF-8', async () => {
  await assert.rejects(boundedJSON(new Response('{}', { headers: { 'content-type': 'application/json', 'content-length': String(LIMITS.response + 1) } })), code('oversized'));
  await assert.rejects(boundedJSON(new Response(JSON.stringify({ text: 'x'.repeat(LIMITS.response) }), { headers: { 'content-type': 'application/json' } })), code('oversized'));
  for (const body of ['invalid', '[]', new Uint8Array([0xff])]) await assert.rejects(boundedJSON(new Response(body, { headers: { 'content-type': 'application/json' } })), code('incompatible_response'));
});

test('protocol and required capabilities fail closed', () => {
  const hello = { server_id: 'server_1', api_major: 1, api_minor: 0, capabilities: ['projects', 'sessions', 'screen', 'events'], engine_epoch: 'engine_1', max_body_bytes: 16384, max_response_bytes: 262144 };
  assert.equal(validateHello(hello, 'server_1'), hello);
  assert.throws(() => validateHello({ ...hello, api_major: 2 }, 'server_1'), code('incompatible_protocol'));
  assert.throws(() => validateHello({ ...hello, capabilities: ['projects'] }, 'server_1'), code('missing_capability'));
  assert.throws(() => validateHello({ ...hello, required_capabilities: ['arbitrary_rpc'] }, 'server_1'), code('missing_capability'));
  assert.throws(() => validateHello(hello, 'wrong_server'), code('server_identity_mismatch'));
});

test('cursor ignores duplicates, detects gaps/restarts and rejects unsafe integers', () => {
  const cursor = new EventCursor(); assert.equal(cursor.accept('stream', 9), true);
  assert.equal(cursor.accept('stream', 9), false); assert.equal(cursor.accept('stream', 8), false);
  assert.throws(() => cursor.accept('stream', 11), code('sequence_gap'));
  assert.throws(() => cursor.accept('new_stream', 10), code('sequence_gap'));
  assert.deepEqual(cursor.value, { stream_id: 'stream', sequence: 9 });
  assert.throws(() => cursor.accept('stream', Number.MAX_SAFE_INTEGER + 1), code('incompatible_response'));
  cursor.reset(); assert.equal(cursor.accept('new_stream', 1), true);
});

test('a paired client lists local and SSH projects, views without control, takes control and sends one prompt', async t => {
  const { client, fixture } = await setup(t);
  assert.equal((await client.projects()).length, 2);
  const sessions = await client.sessions(); assert.equal(sessions.length, 2); assert.equal(sessions[0].host, null); assert.match(sessions[1].host, /SSH/u);
  for (const session of sessions) {
    const screen = await client.screen(session.id); assert.equal(screen.control.owner.role, 'desktop');
    assert.throws(() => client.sendPrompt(session, screen, 'Hello'), code('stale_controller'));
    assert.equal(fixture.stats.acquisitions, sessions.indexOf(session));
    await client.takeControl(session, screen);
    const controlled = await client.screen(session.id); assert.equal(controlled.control.owner.id, client.device.id);
    await client.sendPrompt(session, controlled, 'Review the change.');
    assert.ok((await client.screen(session.id)).screen_sequence > screen.screen_sequence);
  }
  assert.equal(fixture.stats.prompts, 2);
});

test('screen identity, cursor geometry, bounds and controller numbers are validated', async t => {
  const { client } = await setup(t); const screen = await client.screen('session_local');
  for (const invalid of [{ cols: 0 }, { rows: 513 }, { cols: 512, rows: 512 }, { cursor_col: 80 }, { cursor_row: 24 }, { screen_sequence: Number.MAX_SAFE_INTEGER + 1 }, { text: 'x\n'.repeat(30) }, { text: 'x'.repeat(262145) }]) assert.throws(() => validateScreen({ ...screen, ...invalid }, 'session_local'));
  assert.throws(() => validateScreen(screen, 'session_remote'), code('session_changed'));
});

test('pairing a wrong server does not consume enrollment; payload is single use', async t => {
  const fixture = await createFixture(); t.after(() => fixture.close());
  const payload = fixture.issuePairing(), client = new CompanionClient({ currentOrigin: fixture.origin }); t.after(() => client.forget());
  await assert.rejects(client.pair({ ...payload, server_id: 'wrong' }, 'Phone'), code('server_identity_mismatch'));
  await client.pair(payload, 'Phone');
  const duplicate = new CompanionClient({ currentOrigin: fixture.origin }); t.after(() => duplicate.forget());
  await assert.rejects(duplicate.pair(payload, 'Another phone'), code('pairing_expired'));
});

test('lifecycle requires confirmation and scopes, revision and control metadata', async t => {
  const { client, fixture } = await setup(t);
  let session = await client.session('session_local'), screen = await client.screen(session.id);
  assert.throws(() => client.action(session, 'terminate'), code('unavailable'));
  assert.throws(() => client.action(session, 'daemon.shutdown', { confirmed: true }), code('unavailable'));
  assert.ok(confirmation('terminate', { ...session, host: 'desktop' }).description.includes('Unfinished work'));
  await client.action(session, 'rename', { confirmed: true, title: 'Renamed fixture', screen });
  await assert.rejects(client.action(session, 'archive', { confirmed: true, screen }), code('stale_revision'));
  session = await client.session(session.id);
  await client.action(session, 'hibernate', { confirmed: true, screen });
  assert.equal((await client.session(session.id)).hibernated, true); assert.equal(fixture.stats.actions, 2);
  client.device.scopes = ['read']; assert.throws(() => client.action(session, 'terminate', { confirmed: true, screen }), code('forbidden'));
});

test('a lost prompt response is uncertain and reconnect never repeats the mutation', async t => {
  const { client, fixture } = await setup(t); const session = await client.session('session_local');
  await client.takeControl(session, await client.screen(session.id)); const screen = await client.screen(session.id);
  fixture.setMode('drop_mutation'); await assert.rejects(client.sendPrompt(session, screen, 'One prompt only.'), code('mutation_unknown'));
  assert.equal(fixture.stats.prompts, 1);
  fixture.setMode('online'); client.disconnect(); await client.connect();
  const after = await client.screen(session.id); assert.equal(after.incarnation, screen.incarnation); assert.equal(after.control.command_seq, 1); assert.equal(fixture.stats.prompts, 1);
  await assert.rejects(client.sendPrompt(session, screen, 'A stale duplicate'), code('mutation_unknown'));
  assert.equal(fixture.stats.prompts, 1);
});

test('desktop controller changes fence stale prompt and lifecycle writes', async t => {
  const { client, fixture } = await setup(t); const session = await client.session('session_local');
  await client.takeControl(session, await client.screen(session.id)); const screen = await client.screen(session.id);
  fixture.setMode('desktop'); await assert.rejects(client.sendPrompt(session, screen, 'Stale input'), code('stale_controller'));
  assert.equal(fixture.stats.prompts, 0); assert.equal((await client.screen(session.id)).control.owner.role, 'desktop');
});

test('revoke invalidates the next request and never leaves a usable client credential', async t => {
  const { client, fixture } = await setup(t); fixture.setMode('revoked');
  await assert.rejects(client.sessions(), code('unauthorized')); assert.equal(client.paired, false); assert.equal(client.device, null);
  assert.ok(!JSON.stringify(client).includes('Bearer'));
});

test('own-device revocation and forgetting erase enrollment material', async t => {
  const { client } = await setup(t); await client.revoke(); assert.equal(client.paired, false); assert.equal(client.hello, null); assert.equal(client.cursor.value, null);
});

test('WebSocket authenticates without URL secrets and reconnect supplies acknowledged cursor', async t => {
  const { client, fixture } = await setup(t); let events = 0; const errors = [];
  client.subscribe(() => events++, error => errors.push(error.code)); await waitFor(() => events > 0);
  const cursor = copyCursor(client.cursor.value); client.disconnect(); await client.connect();
  client.subscribe(() => events++, error => errors.push(error.code)); await waitFor(() => fixture.stats.subscriptions === 2);
  assert.deepEqual(fixture.stats.replayCursors[1], cursor); assert.deepEqual(errors, []);
  fixture.setMode('revoked'); await waitFor(() => errors.includes('revoked')); assert.equal(client.paired, false);
});
const copyCursor = value => structuredClone(value);

test('request deadlines abort reads and disconnect cancels in-flight responses', async t => {
  let started;
  const { client } = await setup(t, { fetch: async (url, options) => {
    if (url.endsWith('/screen')) return new Promise((resolve, reject) => { options.signal.addEventListener('abort', () => reject(new Error('aborted'))); started?.(); });
    return fetch(url, options);
  } });
  client.deadline = 30;
  await assert.rejects(client.screen('session_local'), code('timeout'));
  client.deadline = 10000;
  const requestStarted = new Promise(resolve => { started = resolve; });
  const pending = client.screen('session_local'); await requestStarted; client.disconnect();
  await assert.rejects(pending, code('timeout'));
});

test('fixture rejects foreign origins, arbitrary paths, unauthorized API and hidden files', async t => {
  const { fixture } = await setup(t);
  assert.equal((await fetch(fixture.origin + '/v1/sessions')).status, 401);
  assert.equal((await fetch(fixture.origin + '/', { headers: { Origin: 'https://evil.example' } })).status, 403);
  assert.equal((await fetch(fixture.origin + '/.git/config')).status, 401);
  assert.equal((await fetch(fixture.origin + '/v1/rpc')).status, 401);
});

test('resync events validate and acknowledge their cursor before projections are invalidated', async t => {
  let socket;
  class FakeSocket {
    constructor(url) { assert.ok(!url.includes('token')); this.url = url; socket = this; queueMicrotask(() => this.onopen?.()); }
    send() {} close() {}
  }
  const { client } = await setup(t, { WebSocket: FakeSocket }); const events = [], errors = [];
  client.subscribe(event => events.push(event.kind), error => errors.push(error.code));
  socket.onmessage({ data: JSON.stringify({ cursor: { stream_id: 'events', sequence: 7 }, kind: 'resync_required' }) });
  assert.deepEqual(client.cursor.value, { stream_id: 'events', sequence: 7 }); assert.deepEqual(events, ['resync_required']);
  socket.onmessage({ data: JSON.stringify({ cursor: { stream_id: 'events', sequence: Number.MAX_SAFE_INTEGER + 1 }, kind: 'resync_required' }) });
  assert.deepEqual(client.cursor.value, { stream_id: 'events', sequence: 7 }); assert.deepEqual(errors, ['incompatible_response']);
});

test('gateway command-sequence or unconfirmed delivery errors remain uncertain', async t => {
  let failure;
  const { client } = await setup(t, { fetch: async (url, options) => {
    if (url.endsWith('/text') && failure) return new Response(JSON.stringify({ code: failure }), { status: 409, headers: { 'content-type': 'application/json' } });
    return fetch(url, options);
  } });
  const session = await client.session('session_local'); await client.takeControl(session, await client.screen(session.id)); const screen = await client.screen(session.id);
  for (const value of ['command_sequence', 'input_unconfirmed', 'outcome_unknown', 'replayed_mutation']) { failure = value; await assert.rejects(client.sendPrompt(session, screen, 'Synthetic content'), code('mutation_unknown')); }
});

test('gateway WebSocket connect deadline bounds a stalled handshake', async t => {
  class StalledSocket { close() {} }
  const { client } = await setup(t, { WebSocket: StalledSocket }); const errors = [];
  client.deadline = 100;
  client.subscribe(() => {}, error => errors.push(error.code));
  await waitFor(() => errors.length > 0); assert.deepEqual(errors, ['timeout']);
});
