// Optional browser tests use an existing Chromium binary and its debugging pipe.
// No package installation, exposed debug port, profile reuse, or external network.
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

export async function launchBrowser(executable) {
  const profile = await mkdtemp(join(tmpdir(), 'zeus-companion-browser-'));
  const process = spawn(executable, ['--headless', '--no-sandbox', '--disable-gpu', '--disable-background-networking', '--disable-component-update', '--no-first-run', '--remote-debugging-pipe', `--user-data-dir=${profile}`], { stdio: ['ignore', 'ignore', 'ignore', 'pipe', 'pipe'] });
  let sequence = 0, buffer = Buffer.alloc(0), sessionId;
  const pending = new Map();
  const send = (method, params = {}, session = sessionId) => new Promise((resolve, reject) => {
    const id = ++sequence;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`Browser command timed out: ${method}`)); }, 10000);
    pending.set(id, { resolve, reject, timer });
    process.stdio[3].write(JSON.stringify({ id, method, params, ...(session ? { sessionId: session } : {}) }) + '\0');
  });
  process.stdio[4].on('data', chunk => {
    buffer = Buffer.concat([buffer, chunk]);
    let end;
    while ((end = buffer.indexOf(0)) !== -1) {
      const message = JSON.parse(buffer.subarray(0, end)); buffer = buffer.subarray(end + 1);
      const item = pending.get(message.id);
      if (item) { pending.delete(message.id); clearTimeout(item.timer); if (message.error) item.reject(new Error('Browser command failed')); else item.resolve(message.result); }
    }
  });
  process.on('error', () => { for (const item of pending.values()) { clearTimeout(item.timer); item.reject(new Error('Chromium could not start')); } pending.clear(); });
  const close = async () => {
    process.kill('SIGTERM');
    await new Promise(resolve => { if (process.exitCode !== null) resolve(); else { const timer = setTimeout(() => { process.kill('SIGKILL'); resolve(); }, 3000); process.once('exit', () => { clearTimeout(timer); resolve(); }); } });
    for (const item of pending.values()) { clearTimeout(item.timer); item.reject(new Error('Browser closed')); } pending.clear();
    await rm(profile, { recursive: true, force: true });
  };
  try {
    const version = await send('Browser.getVersion');
    const target = await send('Target.createTarget', { url: 'about:blank' });
    sessionId = (await send('Target.attachToTarget', { targetId: target.targetId, flatten: true })).sessionId;
    await send('Page.enable'); await send('Runtime.enable');
    return {
      version: version.product,
      send,
      async evaluate(expression) {
        const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true, userGesture: true });
        if (result.exceptionDetails) throw new Error('Browser evaluation failed');
        return result.result.value;
      },
      async wait(expression, timeout = 5000) {
        const deadline = Date.now() + timeout;
        while (Date.now() < deadline) {
          if (await this.evaluate(expression)) return;
          await new Promise(resolve => setTimeout(resolve, 25));
        }
        throw new Error(`Browser assertion timed out: ${expression}`);
      },
      async navigate(url) { await send('Page.navigate', { url }); await this.wait('document.readyState === "complete"'); },
      async viewport(width, height) { await send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: true }); },
      close,
    };
  } catch (error) { await close(); throw error; }
}
