import { ACTIONS, CAPABILITIES, CompanionError, EventCursor, LIMITS, array, bytes, identifier, integer, object, origin, promptText, requireThat, routeId, string, terminalText } from './model.js';

export async function boundedJSON(response, limit = LIMITS.response) {
  requireThat(response.headers.get('content-type')?.includes('application/json'), 'incompatible_response');
  const length = response.headers.get('content-length');
  requireThat(length === null || (Number.isSafeInteger(Number(length)) && Number(length) <= limit), 'oversized');
  requireThat(response.body, 'incompatible_response');
  const reader = response.body.getReader();
  const decoder = new TextDecoder('utf-8', { fatal: true });
  let size = 0, text = '';
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      requireThat(size <= limit, 'oversized');
      text += decoder.decode(value, { stream: true });
    }
    text += decoder.decode();
    return object(JSON.parse(text));
  } catch (error) {
    await reader.cancel().catch(() => {});
    throw error instanceof CompanionError ? error : new CompanionError('incompatible_response');
  } finally { reader.releaseLock(); }
}

export function validateHello(value, expectedServer) {
  object(value); requireThat(value.api_major === 1, 'incompatible_protocol'); integer(value.api_minor);
  requireThat(string(value.server_id, 200) === expectedServer, 'server_identity_mismatch');
  const capabilities = array(value.capabilities, 64).map(value => string(value, 80));
  requireThat(CAPABILITIES.every(capability => capabilities.includes(capability)), 'missing_capability');
  if (value.required_capabilities !== undefined) requireThat(array(value.required_capabilities, 64).every(capability => [...CAPABILITIES, 'send_text', 'control_lease', 'rename', 'lifecycle', 'scrollback'].includes(capability)), 'missing_capability');
  identifier(value.engine_epoch); integer(value.max_body_bytes, 16 * 1024); integer(value.max_response_bytes, LIMITS.response);
  requireThat(value.max_body_bytes > 0 && value.max_response_bytes > 0);
  return value;
}

export function validateProject(value) {
  object(value); routeId(value.id); string(value.name, 512); string(value.root, 2048);
  if (value.host !== null) string(value.host, 512);
  return value;
}
export function validateSession(value) {
  object(value); routeId(value.id); routeId(value.project_id); string(value.kind, 100); string(value.title, 512, true); string(value.cwd, 2048);
  if (value.host !== null) string(value.host, 512);
  string(value.status, 100); string(value.revision, 200);
  for (const field of ['created_at_ms', 'updated_at_ms']) requireThat(Number.isFinite(value[field]) && value[field] >= 0 && value[field] <= 8.64e15);
  for (const field of ['archived', 'hibernated']) requireThat(typeof value[field] === 'boolean');
  return value;
}
export function validateControl(value) {
  object(value); object(value.epoch); identifier(value.epoch.incarnation); integer(value.epoch.generation); integer(value.command_seq);
  if (value.owner !== null) { object(value.owner); identifier(value.owner.id); string(value.owner.label, 128); requireThat(['desktop', 'mobile'].includes(value.owner.role)); }
  return value;
}
export function validateScreen(value, sessionId) {
  object(value); requireThat(routeId(value.session_id) === sessionId, 'session_changed'); identifier(value.incarnation); integer(value.screen_sequence);
  integer(value.cols, LIMITS.cols); integer(value.rows, LIMITS.rows);
  requireThat(value.cols > 0 && value.rows > 0 && value.cols * value.rows <= LIMITS.cells);
  integer(value.cursor_row, value.rows - 1); integer(value.cursor_col, value.cols - 1);
  validateControl(value.control);
  for (const field of ['exited', 'truncated']) requireThat(typeof value[field] === 'boolean');
  const text = terminalText(value.text);
  const lines = text.split('\n');
  // The projection may trim trailing rows or end with a newline; it cannot exceed the grid.
  if (lines.at(-1) === '') lines.pop();
  requireThat(lines.length <= value.rows && lines.every(line => bytes(line) <= value.cols * 32));
  return { ...value, text };
}

// Deliberately exposes named operations only. No generic RPC, arbitrary path, or raw input.
export class CompanionClient {
  #fetch; #WebSocket; #origin; #token = null; #server = null; #controllers = new Set(); #socket = null; #serial = 0;
  #mutationPending = false;
  constructor({ currentOrigin = globalThis.location?.origin, fetch = globalThis.fetch, WebSocket = globalThis.WebSocket, deadline = LIMITS.deadline } = {}) {
    this.#origin = origin(currentOrigin, currentOrigin); this.#fetch = fetch; this.#WebSocket = WebSocket; this.deadline = deadline;
    this.device = null; this.hello = null; this.cursor = new EventCursor();
    this.metrics = { requests: 0, responseBytesLimit: LIMITS.response, requestMs: [], reconnects: 0, events: 0 };
  }
  get paired() { return this.#token !== null; }
  hasScope(scope) { return this.device?.scopes.includes(scope) ?? false; }
  hasCapability(capability) { return this.hello?.capabilities.includes(capability) ?? false; }
  disconnect() {
    this.#serial++;
    for (const controller of this.#controllers) controller.abort();
    this.#controllers.clear();
    if (this.#socket) { this.#socket.onclose = null; this.#socket.onerror = null; this.#socket.onmessage = null; this.#socket.close(); this.#socket = null; }
  }
  forget() { this.disconnect(); this.#token = null; this.#server = null; this.device = null; this.hello = null; this.cursor.reset(); }
  async #request(path, body, authenticate = true) {
    if (authenticate) requireThat(this.#token, 'unauthorized');
    const serial = this.#serial;
    const controller = new AbortController(); this.#controllers.add(controller);
    const timer = setTimeout(() => controller.abort(), this.deadline);
    const started = performance.now();
    try {
      const headers = { Accept: 'application/json' };
      if (authenticate) headers.Authorization = `Bearer ${this.#token}`;
      let encoded;
      if (body !== undefined) { encoded = JSON.stringify(body); requireThat(bytes(encoded) <= (this.hello?.max_body_bytes ?? 16 * 1024), 'oversized'); headers['Content-Type'] = 'application/json'; }
      this.metrics.requests++;
      const response = await this.#fetch(`${this.#origin}${path}`, { method: body === undefined ? 'GET' : 'POST', headers, body: encoded, signal: controller.signal, credentials: 'omit', cache: 'no-store', redirect: 'error', referrerPolicy: 'no-referrer' });
      const value = await boundedJSON(response, this.hello?.max_response_bytes ?? LIMITS.response);
      requireThat(serial === this.#serial, 'offline');
      if (!response.ok) {
        const code = typeof value.code === 'string' && /^[a-z_]{1,80}$/.test(value.code) ? value.code : 'unavailable';
        if (['unauthorized', 'revoked', 'expired'].includes(code)) this.forget();
        throw new CompanionError(code);
      }
      return value;
    } catch (error) {
      if (error instanceof CompanionError) throw error;
      throw new CompanionError(controller.signal.aborted ? 'timeout' : 'offline');
    } finally {
      clearTimeout(timer); this.#controllers.delete(controller);
      this.metrics.requestMs.push(Math.round((performance.now() - started) * 100) / 100);
      if (this.metrics.requestMs.length > 128) this.metrics.requestMs.shift();
    }
  }
  async pair(payload, deviceName) {
    requireThat(!this.paired, 'unavailable'); origin(payload.origin, this.#origin);
    string(deviceName, 80); requireThat(deviceName.trim().length > 0 && !/[\u0000-\u001f\u007f]/u.test(deviceName));
    requireThat(payload.expires_at_ms > Date.now(), 'pairing_expired');
    const response = await this.#request('/v1/pair', { api_major: 1, code: payload.code, device_name: deviceName.trim(), expected_server_id: payload.server_id }, false);
    try {
      requireThat(response.server_id === payload.server_id, 'server_identity_mismatch');
      routeId(response.device_id); string(response.token, 512); integer(response.expires_at_ms); requireThat(response.expires_at_ms > Date.now(), 'expired');
      const scopes = array(response.scopes, 4); requireThat(scopes.every(scope => ['read', 'interact', 'spawn', 'lifecycle'].includes(scope)) && scopes.includes('read'), 'forbidden');
      this.#server = payload.server_id; this.#token = response.token;
      this.device = { id: response.device_id, name: deviceName.trim(), scopes, expires_at_ms: response.expires_at_ms };
      await this.connect();
    } catch (error) { this.forget(); throw error; }
    return this.device;
  }
  async connect() {
    requireThat(this.device?.expires_at_ms > Date.now(), 'expired');
    const hello = validateHello(await this.#request('/v1/hello'), this.#server);
    if (this.hello && this.hello.engine_epoch !== hello.engine_epoch) this.cursor.reset();
    this.hello = hello; this.metrics.reconnects++;
    return hello;
  }
  #epoch(value) { requireThat(value.engine_epoch === this.hello?.engine_epoch, 'session_changed'); }
  async #list(path, validate) {
    const items = [], seen = new Set(); let offset = 0;
    do {
      const page = await this.#request(`${path}?offset=${offset}&limit=${LIMITS.page}`); this.#epoch(page);
      for (const item of array(page.items, LIMITS.page)) { validate(item); requireThat(!seen.has(item.id)); seen.add(item.id); items.push(item); }
      requireThat(items.length <= LIMITS.list, 'oversized');
      if (page.next_offset === null) break;
      integer(page.next_offset); requireThat(page.next_offset > offset && items.length < LIMITS.list); offset = page.next_offset;
    } while (true);
    return items;
  }
  projects() { return this.#list('/v1/projects', validateProject); }
  sessions() { return this.#list('/v1/sessions', validateSession); }
  async session(id) { const result = await this.#request(`/v1/sessions/${routeId(id)}`); this.#epoch(result); const value = validateSession(result.session); requireThat(value.id === id, 'session_changed'); return value; }
  async screen(id) { return validateScreen(await this.#request(`/v1/sessions/${routeId(id)}/screen`), id); }
  async #once(path, body, validate) {
    requireThat(!this.#mutationPending, 'unavailable');
    this.#mutationPending = true;
    try {
      return validate(await this.#request(path, body));
    } catch (error) {
      if (['offline', 'timeout', 'incompatible_response', 'oversized', 'outcome_unknown'].includes(error.code)) throw new CompanionError('mutation_unknown');
      throw error;
    } finally { this.#mutationPending = false; }
  }
  action(session, kind, { confirmed = false, title, screen } = {}) {
    requireThat(ACTIONS.includes(kind) && confirmed, 'unavailable');
    requireThat(this.hasScope('lifecycle') && this.hasCapability(kind === 'rename' ? 'rename' : 'lifecycle'), 'forbidden');
    validateSession(session);
    if (screen) validateScreen(screen, session.id);
    const mutation_id = crypto.randomUUID();
    const action = kind === 'rename' ? { kind, title: string(title, 160) } : { kind, confirmed: true };
    return this.#once(`/v1/sessions/${routeId(session.id)}/actions`, { engine_epoch: this.hello.engine_epoch, mutation_id, expected_revision: session.revision, expected_control: screen?.control.epoch ?? null, action }, result => { requireThat(result.mutation_id === mutation_id && result.applied === true); return result; });
  }
  takeControl(session, screen) {
    requireThat(this.hasScope('interact') && this.hasCapability('control_lease'), 'forbidden'); validateScreen(screen, session.id);
    return this.#once(`/v1/sessions/${routeId(session.id)}/control/acquire`, { expected: screen.control.epoch, takeover: true }, validateControl);
  }
  sendPrompt(session, screen, text) {
    requireThat(this.hasScope('interact') && this.hasCapability('send_text'), 'forbidden'); validateScreen(screen, session.id);
    requireThat(screen.control.owner?.id === this.device.id && screen.control.owner.role === 'mobile', 'stale_controller');
    return this.#once(`/v1/sessions/${routeId(session.id)}/text`, { expected: screen.control.epoch, command_seq: integer(screen.control.command_seq + 1), text: promptText(text), submit: true }, validateControl);
  }
  async revoke() { requireThat(this.paired, 'unauthorized'); await this.#request(`/v1/devices/${routeId(this.device.id)}/revoke`, {}); this.forget(); }
  subscribe(onEvent, onError) {
    requireThat(this.paired && this.hello, 'unauthorized');
    if (this.#socket) { this.#socket.onclose = null; this.#socket.close(); }
    const serial = this.#serial;
    const socket = new this.#WebSocket(this.#origin.replace(/^http/u, 'ws') + '/v1/events'); this.#socket = socket;
    let failed = false;
    const fail = code => { if (failed || serial !== this.#serial || this.#socket !== socket) return; failed = true; socket.onclose = null; socket.close(); onError(new CompanionError(code)); };
    socket.onopen = () => { if (serial !== this.#serial || !this.paired) { socket.close(); return; } socket.send(JSON.stringify({ api_major: 1, token: this.#token, cursor: this.cursor.value })); };
    socket.onerror = () => fail('offline');
    socket.onclose = () => fail('offline');
    socket.onmessage = message => {
      if (serial !== this.#serial || this.#socket !== socket) return;
      try {
        requireThat(typeof message.data === 'string' && bytes(message.data) <= LIMITS.response, 'oversized');
        const event = object(JSON.parse(message.data));
        if (event.code) { const code = string(event.code, 80); if (['revoked', 'unauthorized', 'expired'].includes(code)) this.forget(); onError(new CompanionError(code)); return; }
        requireThat(['changed', 'resync_required', 'engine_unavailable'].includes(event.kind)); object(event.cursor);
        if (event.kind === 'resync_required') { this.cursor.reset(); onEvent(event); return; }
        if (event.kind === 'engine_unavailable') { fail('offline'); return; }
        if (this.cursor.accept(event.cursor.stream_id, event.cursor.sequence)) { this.metrics.events++; onEvent(event); }
      } catch (error) { fail(error instanceof CompanionError ? error.code : 'incompatible_response'); }
    };
    return () => { if (this.#socket === socket) { socket.onclose = null; socket.close(); this.#socket = null; } };
  }
}
