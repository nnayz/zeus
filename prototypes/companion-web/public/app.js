import { CompanionClient } from './client.js';
import { ACTIONS, CompanionError, confirmation, errorMessage, pairingPayload, requireThat, terminalText } from './model.js';

const $ = id => document.getElementById(id);
const client = new CompanionClient();
const state = { phase: 'unpaired', projects: [], sessions: [], selected: null, screen: null, pendingPair: null, dialog: null, hidden: false, busy: false, uncertain: false, generation: 0 };
let updateTimer = null, refreshPromise = null, dirty = false;
const labels = { unpaired: 'Not paired', verifying: 'Verify identity', connecting: 'Connecting', online: 'Connected', offline: 'Offline · stale view', reconnecting: 'Reconnecting', revoked: 'Device revoked', incompatible: 'Incompatible', hidden: 'Screen hidden' };
const notice = text => { $('notice').textContent = text; };
const safe = value => terminalText(String(value ?? ''), 4096);
const host = session => session.host ?? 'This computer';
function facts(target, pairs) {
  target.replaceChildren(...pairs.flatMap(([label, value]) => { const dt = document.createElement('dt'), dd = document.createElement('dd'); dt.textContent = label; dd.textContent = safe(value); return [dt, dd]; }));
}
function selected() { return state.sessions.find(session => session.id === state.selected); }
function canMutate() { return state.phase === 'online' && !state.busy && !state.hidden && !state.uncertain; }
function ownsControl() { return state.screen?.control.owner?.id === client.device?.id && state.screen?.control.owner?.role === 'mobile'; }
function clearSensitiveDOM() {
  for (const id of ['projects', 'session-facts', 'identity-facts', 'screen', 'output', 'session-title', 'device-label', 'controller']) $(id).replaceChildren();
  for (const id of ['payload', 'prompt', 'rename']) $(id).value = '';
  state.pendingPair = null;
  $('confirmation').close(); state.dialog = null;
}
function render() {
  $('connection').textContent = labels[state.phase] ?? state.phase;
  $('pairing').hidden = client.paired || state.phase === 'verifying' || state.hidden;
  $('identity').hidden = state.phase !== 'verifying' || state.hidden;
  $('workspace').hidden = !client.paired || state.hidden;
  $('pair-form').querySelector('button').disabled = state.busy;
  $('confirm-pair').disabled = state.busy;
  $('privacy-cover').hidden = !state.hidden;
  document.body.classList.toggle('masked', state.hidden);
  if (state.hidden || !client.paired) return;
  $('device-label').textContent = `${client.device.name} · ${client.hasScope('interact') ? 'Interaction permitted' : 'Read-only device'}`;
  $('session-count').textContent = String(state.sessions.length);
  const focusedSession = document.activeElement?.dataset?.session;
  const nodes = [];
  for (const project of state.projects) {
    const title = document.createElement('h3'); title.className = 'project-title'; title.textContent = safe(project.name); nodes.push(title);
    const hint = document.createElement('p'); hint.className = 'hint'; hint.textContent = safe(`${project.host ?? 'This computer'} · ${project.root}`); nodes.push(hint);
    for (const session of state.sessions.filter(session => session.project_id === project.id)) {
      const button = document.createElement('button'); button.type = 'button'; button.className = 'session-row'; button.dataset.session = session.id;
      button.setAttribute('aria-pressed', String(session.id === state.selected));
      const title = document.createElement('strong'); title.textContent = safe(session.title || 'Untitled session');
      const details = document.createElement('small'); details.textContent = safe(`${session.kind} · ${session.status} · ${host(session)}`);
      button.append(title, details); button.addEventListener('click', () => void openSession(session.id)); nodes.push(button);
    }
  }
  if (!nodes.length) { const empty = document.createElement('p'); empty.textContent = 'No projects are available for this device.'; nodes.push(empty); }
  $('projects').replaceChildren(...nodes);
  if (focusedSession) [...$('projects').querySelectorAll('button')].find(button => button.dataset.session === focusedSession)?.focus({ preventScroll: true });
  const session = selected(); $('detail').hidden = !session;
  if (!session) return;
  $('session-title').textContent = safe(session.title || 'Untitled session');
  facts($('session-facts'), [['Agent', session.kind], ['Status', session.status], ['Machine', host(session)], ['Directory', session.cwd], ['Session ID', session.id], ['Updated', new Date(session.updated_at_ms).toLocaleString()]]);
  const screen = state.screen;
  $('screen').textContent = screen?.text ?? 'Loading authoritative screen…';
  $('screen-sequence').textContent = screen ? `#${screen.screen_sequence} · ${screen.cols} × ${screen.rows}${screen.truncated ? ' · truncated' : ''}` : '';
  $('controller').textContent = !screen ? 'Read-only · waiting for controller state' : ownsControl() ? 'You have control · prompt submission enabled while connected' : screen.control.owner ? `Read-only · controlled by ${safe(screen.control.owner.label)} (${screen.control.owner.role})` : 'Read-only · no active controller';
  $('take-control').hidden = !client.hasScope('interact') || !client.hasCapability('control_lease') || ownsControl();
  $('take-control').disabled = !canMutate() || !screen || screen.exited;
  $('send-prompt').disabled = !canMutate() || !ownsControl() || !client.hasScope('interact') || !client.hasCapability('send_text') || screen?.exited;
  $('prompt').disabled = $('send-prompt').disabled;
  $('review-delivery').hidden = !state.uncertain;
  $('review-delivery').disabled = state.phase !== 'online' || state.busy;
  $('load-output').disabled = !client.hasCapability('scrollback') || state.phase !== 'online';
  $('output').textContent = 'Bounded recent output is not advertised by this gateway. The current screen is available above.';
  $('actions').replaceChildren(...ACTIONS.map(action => {
    const button = document.createElement('button'); button.type = 'button'; button.className = action === 'terminate' ? 'quiet danger' : 'quiet'; button.textContent = action[0].toUpperCase() + action.slice(1);
    button.disabled = !canMutate() || !client.hasScope('lifecycle') || !client.hasCapability(action === 'rename' ? 'rename' : 'lifecycle');
    button.addEventListener('click', () => openConfirmation(action)); return button;
  }));
}
function fail(error) {
  const code = error instanceof CompanionError ? error.code : 'unavailable';
  notice(errorMessage(code));
  if (['unauthorized', 'revoked', 'expired'].includes(code)) { client.forget(); state.phase = 'revoked'; state.projects = []; state.sessions = []; state.screen = null; state.selected = null; clearSensitiveDOM(); }
  else if (['incompatible_response', 'incompatible_protocol', 'missing_capability', 'server_identity_mismatch', 'oversized'].includes(code)) { client.forget(); state.phase = 'incompatible'; state.projects = []; state.sessions = []; state.screen = null; clearSensitiveDOM(); }
  else if (code === 'mutation_unknown') { state.uncertain = true; state.phase = 'offline'; client.disconnect(); }
  else if (['offline', 'timeout', 'session_changed', 'sequence_gap', 'stale_controller', 'stale_revision', 'stale_control', 'stale_epoch', 'controller_conflict', 'duplicate_command'].includes(code)) { state.phase = 'offline'; client.disconnect(); }
  render();
}
async function projection() {
  const generation = state.generation;
  const [projects, sessions] = await Promise.all([client.projects(), client.sessions()]);
  let detail = null, screen = null;
  if (state.selected && sessions.some(session => session.id === state.selected)) {
    [detail, screen] = await Promise.all([client.session(state.selected), client.screen(state.selected)]);
  }
  if (generation !== state.generation || state.hidden) return;
  if (screen && state.screen && screen.session_id === state.screen.session_id) {
    if (screen.incarnation !== state.screen.incarnation) { state.uncertain = true; notice('The session incarnation changed. Review the new screen before permitting new actions.'); }
    else requireThat(screen.screen_sequence >= state.screen.screen_sequence, 'sequence_gap');
  }
  state.projects = projects; state.sessions = sessions.map(session => session.id === detail?.id ? detail : session); state.screen = screen;
  if (!detail) state.selected = null;
  render();
}
function scheduleRefresh() {
  if (state.hidden || state.phase !== 'online') return;
  dirty = true;
  if (updateTimer || refreshPromise) return;
  updateTimer = setTimeout(() => {
    updateTimer = null; dirty = false;
    refreshPromise = projection().catch(fail).finally(() => { refreshPromise = null; if (dirty) scheduleRefresh(); });
  }, 150);
}
async function reconnect() {
  if (!client.paired || state.hidden || state.busy) return;
  state.generation++; client.disconnect(); state.phase = 'reconnecting'; render();
  try {
    await client.connect(); await projection();
    if (state.hidden) return;
    state.phase = 'online';
    client.subscribe(event => { if (event.kind === 'resync_required') { state.screen = null; notice('Restoring a fresh authoritative snapshot.'); } scheduleRefresh(); }, error => {
      if (error.code === 'sequence_gap') { client.cursor.reset(); void reconnect(); }
      else fail(error);
    });
    render();
  } catch (error) { fail(error); }
}
async function openSession(id) {
  if (state.busy || state.hidden || state.phase !== 'online') return;
  state.generation++; state.selected = id; state.screen = null; $('prompt').value = ''; render();
  try { await projection(); $('session-title').focus({ preventScroll: true }); } catch (error) { fail(error); }
}
async function runMutation(operation, success) {
  requireThat(canMutate(), 'unavailable');
  state.busy = true; render();
  try { await operation(); notice(success); await projection(); }
  catch (error) { fail(error); }
  finally { state.busy = false; render(); }
}
function openConfirmation(action) {
  const session = selected(); if (!session || !canMutate()) return;
  state.dialog = { action, session: structuredClone(session), screen: structuredClone(state.screen) };
  const copy = action === 'control' ? { title: 'Take Control?', description: `Take control of ${session.title} on ${host(session)}. The current controller will lose permission to send input. Viewing alone does not change ownership.` } : confirmation(action, { ...session, host: host(session) });
  $('confirm-title').textContent = safe(copy.title); $('confirm-description').textContent = safe(copy.description);
  $('rename-label').hidden = action !== 'rename'; $('rename').value = action === 'rename' ? session.title : '';
  $('confirm-action').textContent = action === 'control' ? 'Take Control' : `Confirm ${action}`;
  $('confirmation').showModal();
}

$('pair-form').addEventListener('submit', event => {
  event.preventDefault();
  try {
    const payload = pairingPayload($('payload').value, location.origin);
    const name = $('device-name').value.trim(); requireThat(name.length > 0);
    state.pendingPair = { payload, name }; $('payload').value = ''; state.phase = 'verifying';
    facts($('identity-facts'), [['HTTPS origin', payload.origin], ['Server ID', payload.server_id], ['Device name', name], ['Code expires', new Date(payload.expires_at_ms).toLocaleTimeString()]]);
    notice('No pairing code has been sent yet. Compare the identity with your trusted computer.'); render(); $('confirm-pair').focus();
  } catch (error) { $('payload').value = ''; fail(error); }
});
$('cancel-pair').addEventListener('click', () => { state.pendingPair = null; state.phase = 'unpaired'; $('identity-facts').replaceChildren(); notice('Pairing cancelled.'); render(); $('payload').focus(); });
$('confirm-pair').addEventListener('click', async () => {
  if (!state.pendingPair || state.busy) return;
  const { payload, name } = state.pendingPair; state.pendingPair = null; state.busy = true; state.phase = 'connecting'; render();
  try { await client.pair(payload, name); state.busy = false; notice('Paired. Opening a session is read-only until you explicitly take control.'); await reconnect(); }
  catch (error) { fail(error); }
  finally { payload.code = ''; state.busy = false; $('identity-facts').replaceChildren(); if (!client.paired && state.phase === 'connecting') state.phase = 'unpaired'; render(); }
});
$('refresh').addEventListener('click', () => void reconnect());
$('take-control').addEventListener('click', () => openConfirmation('control'));
$('cancel-action').addEventListener('click', () => $('confirmation').close());
$('confirmation').addEventListener('close', () => { state.dialog = null; $('rename').value = ''; });
$('confirm-action').addEventListener('click', async () => {
  const pending = state.dialog; if (!pending || (pending.action !== 'unpair' && !canMutate())) return;
  const title = $('rename').value.trim(); $('confirmation').close();
  if (pending.action === 'unpair') { await forgetDevice(); return; }
  await runMutation(() => pending.action === 'control' ? client.takeControl(pending.session, pending.screen) : client.action(pending.session, pending.action, { confirmed: true, title, screen: pending.screen }), pending.action === 'control' ? 'Control acquired. The latest controller state is shown below.' : 'Session action confirmed by the gateway.');
});
$('prompt-form').addEventListener('submit', async event => {
  event.preventDefault(); const text = $('prompt').value;
  if ($('send-prompt').disabled) return;
  $('prompt').value = '';
  await runMutation(() => client.sendPrompt(selected(), state.screen, text), 'Prompt accepted. Waiting for the agent’s next update.');
});
$('review-delivery').addEventListener('click', () => { if (state.phase === 'online' && !state.busy) { state.uncertain = false; notice('You reviewed the current state. New actions are enabled; the previous action was not replayed.'); render(); } });
async function forgetDevice() {
  // Local deletion is always possible, including when the gateway is unreachable.
  let revoked = false;
  try { await client.revoke(); revoked = true; } catch { /* Unpair must still delete local data. */ }
  client.forget(); state.generation++; state.projects = []; state.sessions = []; state.selected = null; state.screen = null; state.uncertain = false; state.phase = 'unpaired'; clearSensitiveDOM();
  const registration = await navigator.serviceWorker?.getRegistration();
  registration?.active?.postMessage('delete-shell'); await registration?.unregister();
  notice(revoked ? 'Device revoked. Local credentials and session data deleted.' : 'Local credentials and session data deleted. Server revocation could not be confirmed. Revoke this device on your Zeus computer.'); render();
}
$('unpair').addEventListener('click', () => {
  if (state.busy) return;
  state.dialog = { action: 'unpair' }; $('confirm-title').textContent = 'Revoke and delete local data?'; $('confirm-description').textContent = 'Revoke this device and delete this tab’s credential and session data. If the gateway is unreachable, local deletion still completes and you must revoke the device on your Zeus computer.'; $('rename-label').hidden = true; $('confirm-action').textContent = 'Revoke and delete'; $('confirmation').showModal();
});
function hideScreen() {
  if (state.hidden) return;
  state.hidden = true; state.generation++; client.disconnect(); clearTimeout(updateTimer); updateTimer = null; dirty = false;
  if (state.busy && client.paired) state.uncertain = true;
  state.phase = 'hidden'; clearSensitiveDOM(); render();
}
$('privacy').addEventListener('click', hideScreen);
$('reveal').addEventListener('click', async () => {
  state.hidden = false; state.phase = client.paired ? 'offline' : 'unpaired'; render();
  if (client.paired) await reconnect();
});
document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'hidden') hideScreen(); });
addEventListener('pagehide', hideScreen);
addEventListener('pageshow', event => { if (event.persisted) hideScreen(); });
addEventListener('offline', () => { if (client.paired) fail(new CompanionError('offline')); });
addEventListener('online', () => { if (!state.hidden) void reconnect(); });
if ('serviceWorker' in navigator) navigator.serviceWorker.register('./sw.js').catch(() => { /* Offline shell is optional; never report browser exception content. */ });
render();
