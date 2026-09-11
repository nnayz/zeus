// Explicit opt-in: launch #73's isolated Rust fixture, never a user's gateway.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { constants } from 'node:fs';
import { mkdtemp, open, realpath, rm, stat } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { CompanionClient } from '../public/client.js';
import { pairingPayload } from '../public/model.js';

const binary = process.env.COMPANION_RUST_FIXTURE;
if (!binary) throw new Error('Set COMPANION_RUST_FIXTURE to the built zeus-companion fixture example.');
const directory = await mkdtemp(join(await realpath(tmpdir()), 'zeus-companion-rust-smoke-'));
const enrollment = join(directory, 'enrollment.json');
const fixture = spawn(binary, [enrollment], { stdio: ['pipe', 'ignore', 'ignore'] });
let launchFailed = false, finished = false, client, authorization = null, stage = 'fixture startup';
fixture.on('error', () => { launchFailed = true; });
fixture.once('close', () => { finished = true; });
fixture.stdin.on('error', () => {});
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(check, timeout = 10000) {
  const deadline = Date.now() + timeout;
  while (!(await check())) { if (Date.now() >= deadline) throw new Error('Smoke deadline exceeded'); await delay(20); }
}
try {
  await until(async () => { if (launchFailed || fixture.exitCode !== null) throw new Error('Rust fixture failed to start'); return stat(enrollment).then(() => true, () => false); });
  const file = await open(enrollment, constants.O_RDONLY | constants.O_NOFOLLOW);
  let payload;
  try {
    const info = await file.stat(); assert.ok(info.isFile() && (info.mode & 0o077) === 0 && info.uid === process.getuid() && info.size <= 8192);
    const text = await file.readFile('utf8'), parsed = JSON.parse(text);
    const url = new URL(parsed.origin); assert.ok(['127.0.0.1', '[::1]', 'localhost'].includes(url.hostname));
    payload = pairingPayload(text, parsed.origin);
  } finally { await file.close(); }
  client = new CompanionClient({ currentOrigin: payload.origin, fetch: async (url, options) => {
    if (options.headers.Authorization) authorization = options.headers.Authorization;
    return fetch(url, options);
  } });
  stage = 'pairing and hello'; await client.pair(payload, 'Rust browser-client smoke'); payload.code = '';
  stage = 'project/session discovery'; const projects = await client.projects(), sessions = await client.sessions();
  assert.ok(projects.length > 0); assert.equal(sessions.length, 1);
  let session = sessions[0]; assert.equal(session.title, 'fixture', 'refuse mutations outside the known isolated echo fixture');
  stage = 'screen projection'; const before = await client.screen(session.id); assert.ok(before.text.includes('fixture-ready')); assert.equal(before.exited, false);
  stage = 'explicit control'; await client.takeControl(session, before); const controlled = await client.screen(session.id); assert.equal(controlled.control.owner.id, client.device.id);
  const events = [], errors = [];
  stage = 'event authentication'; client.subscribe(event => events.push(event.kind), error => errors.push(error.code)); await until(() => events.length > 0);
  const cursor = structuredClone(client.cursor.value); assert.ok(cursor);
  stage = 'prompt delivery'; await client.sendPrompt(session, controlled, 'companion-rust-smoke');
  let after;
  await until(async () => { after = await client.screen(session.id); return after.text.includes('companion-rust-smoke'); });
  assert.equal(after.incarnation, before.incarnation); assert.ok(after.screen_sequence > before.screen_sequence);
  assert.equal(after.control.command_seq, controlled.control.command_seq + 1);
  stage = 'lifecycle rename and invalidation'; session = await client.session(session.id);
  const count = events.length; await client.action(session, 'rename', { confirmed: true, title: 'fixture-renamed', screen: after });
  await until(() => events.length > count); assert.equal((await client.session(session.id)).title, 'fixture-renamed');
  stage = 'reconnect'; client.disconnect(); await client.connect(); const reconnected = await client.screen(session.id);
  assert.equal(reconnected.incarnation, before.incarnation); assert.equal(reconnected.control.command_seq, after.control.command_seq);
  client.subscribe(event => events.push(event.kind), error => errors.push(error.code));
  stage = 'duplicate command rejection'; await assert.rejects(client.sendPrompt(session, controlled, 'companion-rust-smoke'), error => error.code === 'mutation_unknown');
  assert.equal((await client.screen(session.id)).control.command_seq, reconnected.control.command_seq);
  stage = 'confirmed live Archive'; session = await client.session(session.id);
  await client.action(session, 'archive', { confirmed: true, screen: await client.screen(session.id) });
  stage = 'server-side revocation'; const origin = payload.origin; await client.revoke();
  const rejected = await fetch(origin + '/v1/sessions', { headers: { Authorization: authorization }, cache: 'no-store', redirect: 'error' });
  assert.equal(rejected.status, 401); authorization = null;
  assert.ok(errors.every(code => ['offline', 'revoked', 'unauthorized'].includes(code)));
  process.stdout.write('PASS: actual Rust gateway + Engine echo PTY; pairing/identity, project/session list, screen, explicit control, prompt/output, events, rename, reconnect without replay, duplicate rejection, live Archive, server-side revocation.\n');
} catch (error) {
  // The stage and allowlisted client error code are safe; never dump payloads.
  process.stderr.write(`FAIL: ${stage}${error.code && /^[a-z_]+$/.test(error.code) ? ` (${error.code})` : ''}\n`);
  process.exitCode = 1;
} finally {
  client?.forget(); authorization = null; fixture.stdin.end();
  try { await until(() => finished, 10000); if (fixture.exitCode !== 0) process.exitCode = 1; } catch { fixture.kill('SIGKILL'); process.exitCode = 1; }
  await rm(directory, { recursive: true, force: true });
}
