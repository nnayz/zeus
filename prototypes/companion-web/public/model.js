export const LIMITS = Object.freeze({ response: 256 * 1024, prompt: 8 * 1024, pairing: 8192, list: 512, page: 64, rows: 512, cols: 512, cells: 32768, recent: 64 * 1024, deadline: 10000 });
export const ACTIONS = Object.freeze(['rename', 'archive', 'wake', 'hibernate', 'terminate']);
export const CAPABILITIES = Object.freeze(['projects', 'sessions', 'screen', 'events']);
const encoder = new TextEncoder();
export const bytes = value => encoder.encode(value).byteLength;

export class CompanionError extends Error {
  constructor(code) {
    const aliases = { version_mismatch: 'incompatible_protocol', pairing_denied: 'invalid_pairing', stale_controller_epoch: 'stale_controller', not_controller: 'stale_controller', controller_busy: 'stale_controller', stale_engine: 'session_changed', engine_unavailable: 'offline', capability_unavailable: 'unavailable', invalid_snapshot: 'incompatible_response', terminal_geometry: 'incompatible_response', projection_limit: 'oversized', invalid_text: 'invalid_prompt' };
    const canonical = aliases[code] ?? code;
    super(canonical); this.name = 'CompanionError'; this.code = canonical;
  }
}
export function requireThat(value, code = 'incompatible_response') { if (!value) throw new CompanionError(code); }
export function object(value) { requireThat(value && typeof value === 'object' && !Array.isArray(value)); return value; }
export function string(value, max = 1024, empty = false) { requireThat(typeof value === 'string' && bytes(value) <= max && (empty || value.length > 0)); return value; }
export function integer(value, max = Number.MAX_SAFE_INTEGER) { requireThat(Number.isSafeInteger(value) && value >= 0 && value <= max); return value; }
export function array(value, max = LIMITS.list) { requireThat(Array.isArray(value) && value.length <= max); return value; }
export function identifier(value) { string(value, 200); requireThat(/^[A-Za-z0-9_.:-]+$/.test(value)); return value; }
export function routeId(value) { string(value, 64); requireThat(/^[A-Za-z0-9_-]+$/.test(value)); return value; }
export function origin(value, currentOrigin) {
  let url;
  try { url = new URL(string(value, 2048)); } catch { throw new CompanionError('invalid_origin'); }
  const loopback = ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname);
  requireThat(url.protocol === 'https:' || (url.protocol === 'http:' && loopback), 'https_required');
  requireThat(!url.username && !url.password && !url.search && !url.hash && url.pathname === '/', 'invalid_origin');
  requireThat(url.origin === currentOrigin, 'same_origin_required');
  return url.origin;
}

// A mobile payload is entered in a form, never a URL fragment or query parameter.
export function pairingPayload(text, currentOrigin, now = Date.now()) {
  string(text, LIMITS.pairing);
  let value;
  try { value = object(JSON.parse(text)); } catch { throw new CompanionError('invalid_pairing'); }
  const serverOrigin = origin(value.origin, currentOrigin);
  const expires = integer(value.expires_at_ms);
  requireThat(expires > now && expires - now <= 10 * 60 * 1000, 'pairing_expired');
  return { origin: serverOrigin, server_id: identifier(value.server_id), code: string(value.code, 512), expires_at_ms: expires };
}

export function promptText(value) {
  string(value, LIMITS.prompt);
  requireThat(value.trim().length > 0, 'empty_prompt');
  // A command-oriented client does not send terminal escapes or raw key sequences.
  requireThat(!/[\u0000-\u0008\u000b-\u001f\u007f]/u.test(value), 'invalid_prompt');
  return value;
}

export function terminalText(value, maximum = LIMITS.response) {
  string(value, maximum, true);
  // Display as inert text. Remove terminal controls and bidi overrides that can spoof UI.
  return value.replace(/[\u0000-\u0008\u000b-\u001f\u007f\u202a-\u202e\u2066-\u2069]/gu, '�');
}

export function errorMessage(code) {
  const known = {
    offline: 'Offline. Reconnect when your private network is available.',
    timeout: 'The gateway did not respond in time. Reconnect to check current state.',
    revoked: 'This device has been revoked. Pair again from your Zeus computer.',
    unauthorized: 'The device credential is no longer accepted. Pair again.',
    expired: 'This device credential has expired. Pair again.',
    incompatible_response: 'The gateway returned an incompatible response. Update the client or gateway.',
    incompatible_protocol: 'The gateway protocol is incompatible. Update the client or gateway.',
    missing_capability: 'The gateway lacks a required capability. Control is unavailable.',
    server_identity_mismatch: 'Server identity does not match the pairing payload. Pairing stopped.',
    same_origin_required: 'Open this prototype at the HTTPS origin shown by your Zeus gateway.',
    https_required: 'Pairing requires HTTPS. HTTP is allowed only for a local loopback fixture.',
    invalid_origin: 'The pairing origin is invalid.',
    invalid_pairing: 'The pairing payload is invalid. Get a new payload from Zeus.',
    pairing_expired: 'The pairing payload has expired or has an invalid lifetime. Get a new one.',
    oversized: 'The gateway response exceeded the client limit. Connection closed.',
    stale_controller: 'Controller ownership changed. Refresh and explicitly take control again.',
    forbidden: 'This device does not have permission for that action.',
    sequence_gap: 'Updates are no longer contiguous. Refreshing the authoritative screen.',
    session_changed: 'This session incarnation changed. Review its current state before sending anything.',
    mutation_unknown: 'Delivery is uncertain. This action will not be retried. Reconnect and inspect the agent before sending another prompt or action.',
    empty_prompt: 'Enter a prompt before sending.',
    invalid_prompt: 'Use plain text; terminal control characters are not supported.',
    unavailable: 'This operation is unavailable on the connected gateway.',
  };
  return known[code] ?? 'The gateway could not complete this operation. Refresh its current state.';
}

export function confirmation(action, session) {
  requireThat(ACTIONS.includes(action), 'unavailable');
  const effects = { rename: 'Change this session’s title.', archive: 'Archive this session and remove it from the active list.', wake: 'Wake this session; its agent may resume work.', hibernate: 'Hibernate this session and suspend its current work.', terminate: 'Terminate the agent process. Unfinished work may be lost.' };
  return { title: `${action[0].toUpperCase()}${action.slice(1)} session?`, description: `${effects[action]}\n\n${session.title} · ${session.host}\n${session.id}` };
}

// Sequence state is per connection stream and is acknowledged only after validation.
export class EventCursor {
  constructor() { this.value = null; }
  reset() { this.value = null; }
  accept(streamId, sequence) {
    identifier(streamId); integer(sequence);
    if (this.value) {
      requireThat(this.value.stream_id === streamId, 'sequence_gap');
      if (sequence <= this.value.sequence) return false;
      requireThat(sequence === this.value.sequence + 1, 'sequence_gap');
    }
    this.value = { stream_id: streamId, sequence };
    return true;
  }
}
