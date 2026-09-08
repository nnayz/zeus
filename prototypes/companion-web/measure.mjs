// Numeric-only fixture measurements. Never report credentials or content.
import { createFixture } from './dev-server.mjs';
import { CompanionClient } from './public/client.js';

const fixture = await createFixture();
const counts = { requestBodyBytes: 0, responseBodyBytes: 0, eventBodyBytes: 0 };
class MeasuredSocket extends WebSocket {
  constructor(url) { super(url); this.addEventListener('message', event => { counts.eventBodyBytes += new TextEncoder().encode(event.data).byteLength; }); }
}
const client = new CompanionClient({ currentOrigin: fixture.origin, WebSocket: MeasuredSocket, fetch: async (url, options) => {
  counts.requestBodyBytes += new TextEncoder().encode(options.body ?? '').byteLength;
  const response = await fetch(url, options);
  const stream = response.body.pipeThrough(new TransformStream({ transform(chunk, controller) { counts.responseBodyBytes += chunk.byteLength; controller.enqueue(chunk); } }));
  return new Response(stream, { status: response.status, headers: response.headers });
} });
const samples = { requestMs: [], promptRoundTripMs: [], eventMs: [], reconnectMs: [] };
const quantiles = values => { const sorted = [...values].sort((a, b) => a - b); return { n: sorted.length, p50: +sorted[Math.floor(sorted.length * .5)].toFixed(3), p90: +sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * .9))].toFixed(3) }; };
try {
  await client.pair(fixture.issuePairing(), 'Measurement fixture');
  const session = await client.session('session_local'); await client.takeControl(session, await client.screen(session.id));
  for (let index = 0; index < 30; index++) {
    let started = performance.now(); const screen = await client.screen(session.id); samples.requestMs.push(performance.now() - started);
    started = performance.now(); await client.sendPrompt(session, screen, 'Synthetic benchmark prompt'); await client.screen(session.id); samples.promptRoundTripMs.push(performance.now() - started);
  }
  let nextEvent;
  client.subscribe(() => { nextEvent?.(); nextEvent = null; }, () => {});
  // Let the initial invalidation complete before measuring subsequent events.
  await new Promise(resolve => { nextEvent = resolve; });
  for (let index = 0; index < 30; index++) {
    const arrived = new Promise(resolve => { nextEvent = resolve; }), started = performance.now(); fixture.emit(); await arrived; samples.eventMs.push(performance.now() - started);
  }
  const cpu = process.cpuUsage(), idleRequests = fixture.stats.requests, idleEvents = client.metrics.events;
  await new Promise(resolve => setTimeout(resolve, 1000));
  const idleCpu = process.cpuUsage(cpu);
  const idleCounts = { requests: fixture.stats.requests - idleRequests, events: client.metrics.events - idleEvents };
  for (let index = 0; index < 10; index++) { const started = performance.now(); client.disconnect(); await client.connect(); await client.sessions(); await client.screen(session.id); samples.reconnectMs.push(performance.now() - started); }
  process.stdout.write(JSON.stringify({ environment: { node: process.version, platform: process.platform, arch: process.arch, scope: 'Loopback Node fixture and JS client in one process; no TLS, Engine, SSH, browser or radio' }, samples: Object.fromEntries(Object.entries(samples).map(([key, value]) => [key, quantiles(value)])), counts, idle: { observedMilliseconds: 1000, ...idleCounts, processCpuMicroseconds: idleCpu.user + idleCpu.system }, processMemoryBytes: process.memoryUsage().rss, notes: 'CPU/RSS include fixture and Node. Battery and radio energy not measured. Body bytes exclude headers, TLS and TCP overhead.' }, null, 2) + '\n');
} finally { client.forget(); await fixture.close(); }
