// Dirijor test sidecar — a Playwright browser pool the daemon drives over stdio.
// One browser process per engine (chromium/webkit/firefox), reused across every
// test; each run gets a cheap isolated context. Transport: newline-delimited
// JSON on stdin/stdout — {id, method, params} → {id, result} | {id, error};
// logs go to stderr so stdout stays pure NDJSON.
//
// Usage: node server.js
//   methods: ping | run | shutdown

"use strict";

const fs = require("fs");
const path = require("path");
const { chromium, webkit, firefox } = require("playwright");
const { handleBrowser, closeAll, openSessionCount } = require("./browser");

const ENGINES = { chromium, webkit, firefox };

// One long-lived browser per engine, launched lazily and reused.
const browsers = {};

async function getBrowser(engine) {
  const type = ENGINES[engine];
  if (!type) throw new Error(`unknown engine: ${engine}`);
  if (!browsers[engine] || !browsers[engine].isConnected()) {
    browsers[engine] = await type.launch({ headless: true });
  }
  return browsers[engine];
}

// ── Step executor ──────────────────────────────────────────────────────────
// Each step is a single-key object, e.g. {click: "#login"} or {type: ["#u","me"]}.

async function runStep(page, step) {
  const [op, arg] = Object.entries(step)[0];
  switch (op) {
    case "goto": await page.goto(arg, { waitUntil: "domcontentloaded" }); break;
    case "click": await page.click(arg); break;
    case "dblclick": await page.dblclick(arg); break;
    case "fill": await page.fill(arg[0], arg[1]); break;
    case "type": await page.locator(arg[0]).pressSequentially(String(arg[1])); break;
    case "press":
      Array.isArray(arg) ? await page.locator(arg[0]).press(arg[1]) : await page.keyboard.press(arg);
      break;
    case "hover": await page.hover(arg); break;
    case "select": await page.selectOption(arg[0], arg[1]); break;
    case "check": await page.check(arg); break;
    case "uncheck": await page.uncheck(arg); break;
    case "drag": await page.dragAndDrop(arg[0], arg[1]); break;
    case "scroll":
      if (Array.isArray(arg)) await page.mouse.wheel(arg[0], arg[1]);
      else await page.locator(arg).scrollIntoViewIfNeeded();
      break;
    case "waitFor":
      typeof arg === "number"
        ? await page.waitForTimeout(arg)
        : await page.waitForSelector(arg, { timeout: 8000 });
      break;
    case "eval": await page.evaluate(arg); break;
    case "assert": await runAssert(page, arg); break;
    default: throw new Error(`unknown step: ${op}`);
  }
}

async function runAssert(page, a) {
  // {text} → body contains text; {selector, visible|text|count} → element checks.
  if (a.text && !a.selector) {
    await page.waitForFunction((t) => document.body.innerText.includes(t), a.text, { timeout: 6000 });
    return;
  }
  if (!a.selector) throw new Error("assert needs a selector or text");
  const loc = page.locator(a.selector);
  if (a.visible !== false) await loc.first().waitFor({ state: "visible", timeout: 6000 });
  if (a.text) {
    const txt = await loc.first().innerText();
    if (!txt.includes(a.text)) throw new Error(`expected "${a.text}" in ${a.selector}, got "${txt.slice(0, 120)}"`);
  }
  if (typeof a.count === "number") {
    const c = await loc.count();
    if (c !== a.count) throw new Error(`expected ${a.count} of ${a.selector}, got ${c}`);
  }
}

// ── Auth hand-off ────────────────────────────────────────────────────────────
// params.auth seeds state into the context before the first navigation so
// authed flows work on engines nobody logged into (webkit/firefox):
//   cookies:      array of Playwright cookie objects; entries without a
//                 url/domain default to the test URL.
//   localStorage: {key: value} scoped to the test URL's origin, or
//                 {origin: {key: value}} for explicit per-origin seeding.

async function applyAuth(context, params) {
  const auth = params.auth;
  if (!auth) return;

  if (Array.isArray(auth.cookies) && auth.cookies.length) {
    const cookies = auth.cookies.map((c) =>
      c.url || c.domain ? c : { ...c, url: params.url });
    await context.addCookies(cookies);
  }

  if (auth.localStorage && typeof auth.localStorage === "object") {
    // Nested form when every value is itself an object; flat form otherwise.
    const nested = Object.values(auth.localStorage).every((v) => v && typeof v === "object");
    let byOrigin;
    if (nested) {
      byOrigin = auth.localStorage;
    } else {
      let origin = "*";
      try { origin = new URL(params.url).origin; } catch {}
      // file:// pages report origin inconsistently across engines — apply everywhere.
      if (origin === "null" || origin === "file://") origin = "*";
      byOrigin = { [origin]: auth.localStorage };
    }
    for (const [origin, kv] of Object.entries(byOrigin)) {
      await context.addInitScript(([o, items]) => {
        if (o === "*" || location.origin === o) {
          for (const [k, v] of items) localStorage.setItem(k, String(v));
        }
      }, [origin, Object.entries(kv)]);
    }
  }
}

// ── One engine's run ─────────────────────────────────────────────────────────

async function runEngine(engine, params) {
  const browser = await getBrowser(engine);
  const contextOpts = { viewport: params.viewport || { width: 1280, height: 800 } };

  // Named profile: persist cookies + localStorage across runs, per engine, so a
  // login in one run carries into the next. Absent → a clean isolated context.
  let stateFile = null;
  if (params.profile) {
    const dir = path.join(params.artifactDir || ".", "profiles");
    fs.mkdirSync(dir, { recursive: true });
    stateFile = path.join(dir, `${params.profile}-${engine}.json`);
    if (fs.existsSync(stateFile)) contextOpts.storageState = stateFile;
  }

  const context = await browser.newContext(contextOpts);
  await applyAuth(context, params);
  const page = await context.newPage();

  const consoleErrors = [];
  page.on("console", (m) => { if (m.type() === "error") consoleErrors.push(m.text().slice(0, 200)); });
  page.on("pageerror", (e) => consoleErrors.push(String(e).slice(0, 200)));

  const result = { pass: true, engine };
  try {
    const steps = [];
    if (params.url) steps.push({ goto: params.url });
    steps.push(...(params.steps || []));

    for (let i = 0; i < steps.length; i++) {
      try {
        await runStep(page, steps[i]);
      } catch (err) {
        result.pass = false;
        result.failedStep = { index: i, step: steps[i], error: String(err.message || err).slice(0, 300) };
        break;
      }
    }

    result.title = await page.title().catch(() => undefined);
    result.url = page.url();
    // Token-cheap observation: a compact ARIA tree with element roles/names.
    try {
      const a11y = await page.locator("body").ariaSnapshot({ timeout: 3000 });
      result.a11y = a11y.length > 6000 ? a11y.slice(0, 6000) + "\n… (truncated)" : a11y;
    } catch { /* a11y snapshot is best-effort */ }

    if (consoleErrors.length) result.consoleErrors = consoleErrors.slice(0, 20);

    // Screenshot only when something failed (or explicitly requested), saved to
    // disk — the caller gets a path, never inline bytes.
    const wantShot = !result.pass || params.observe === "screenshot";
    if (wantShot && params.artifactDir) {
      const file = path.join(params.artifactDir, `${engine}-${Date.now()}.jpg`);
      await page.screenshot({ path: file, type: "jpeg", quality: 55, fullPage: false }).catch(() => {});
      if (fs.existsSync(file)) result.screenshotPath = file;
    }
  } finally {
    // Persist the (possibly updated) auth/session state back to the profile.
    if (stateFile) { try { await context.storageState({ path: stateFile }); } catch {} }
    await context.close().catch(() => {});
  }
  return result;
}

// ── RPC methods ──────────────────────────────────────────────────────────────

async function handle(method, params) {
  switch (method) {
    case "ping":
      // openSessions lets the daemon skip its idle recycle: killing the sidecar
      // out from under a live interactive browser would silently drop the page
      // an agent is mid-way through driving.
      return { ok: true, engines: Object.keys(ENGINES), openSessions: openSessionCount() };
    case "browser":
      return handleBrowser(getBrowser, params);
    case "run": {
      const engines = (params.engines && params.engines.length ? params.engines : ["chromium"])
        .filter((e) => ENGINES[e]);
      if (params.artifactDir) fs.mkdirSync(params.artifactDir, { recursive: true });
      const settled = await Promise.all(
        engines.map((e) =>
          runEngine(e, params).catch((err) => ({
            pass: false, engine: e, error: String(err.message || err).slice(0, 300),
          }))));
      const results = {};
      for (const r of settled) results[r.engine] = r;
      return { pass: settled.every((r) => r.pass), results };
    }
    case "shutdown":
      queueMicrotask(() => shutdown(0));
      return { ok: true };
    default:
      throw new Error(`unknown method: ${method}`);
  }
}

// ── stdio JSON-RPC transport (newline-delimited) ─────────────────────────────

let buf = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buf += chunk;
  let nl;
  while ((nl = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, nl);
    buf = buf.slice(nl + 1);
    if (!line.trim()) continue;
    let req;
    try { req = JSON.parse(line); } catch { continue; }
    handle(req.method, req.params || {})
      .then((result) => process.stdout.write(JSON.stringify({ id: req.id, result }) + "\n"))
      .catch((err) => process.stdout.write(JSON.stringify({ id: req.id, error: String(err.message || err) }) + "\n"));
  }
});
process.stdin.on("end", () => shutdown(0));

async function shutdown(code) {
  // Interactive contexts first, so named profiles get their storage state
  // flushed before the browser process they live in goes away.
  try { await closeAll(); } catch {}
  for (const b of Object.values(browsers)) { try { await b.close(); } catch {} }
  process.exit(code);
}

process.on("SIGTERM", () => shutdown(0));
process.on("SIGINT", () => shutdown(0));
console.error("dirijor test sidecar ready");
