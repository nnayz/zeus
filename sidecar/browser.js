// Interactive per-session browser for Dirijor agents.
//
// `test.run` (server.js) is a *scripted* runner: one call in, pass/fail out, the
// context thrown away. This module is the other half — a browser an agent drives
// a step at a time, the way a person would: open a page, look at it, act on what
// it saw, look again.
//
// The loop is open → snapshot → act by ref → re-snapshot. Snapshots hand back
// stable refs (@e1, @e2) rather than CSS selectors because a model inventing a
// selector is guessing about markup it cannot see, while a ref is a promise the
// page itself made one call ago. Refs are stamped as DOM attributes, so they die
// with the document on navigation — which is the honest behaviour: after a page
// load, the thing @e3 pointed at is genuinely gone.
//
// Refs are deliberately NOT Playwright's private `_snapshotForAI`: that API is
// unstable across versions and chromium-only, and Dirijor's whole browser story
// is "the same flow on webkit and firefox too". A DOM walk works everywhere.

"use strict";

const fs = require("fs");
const path = require("path");

// One live context+page per Dirijor session id. Contexts are cheap; the browser
// process underneath is shared with the test pool, so N agents browsing at once
// still cost one Chrome.
const sessions = new Map();

const CONSOLE_CAP = 200;

// ── Session lifecycle ────────────────────────────────────────────────────────

async function ensureSession(getBrowser, params) {
  const id = params.sessionId;
  if (!id) throw new Error("sessionId is required");

  const existing = sessions.get(id);
  if (existing && !existing.page.isClosed()) {
    if (!params.engine || params.engine === existing.engine) return existing;
    // An explicit engine switch means the agent wants a different renderer;
    // honour it by rebuilding rather than silently ignoring the request.
    await closeSession(id);
  }

  const engine = params.engine || "chromium";
  const browser = await getBrowser(engine);
  const contextOpts = { viewport: params.viewport || { width: 1280, height: 800 } };

  // A named profile keeps cookies and logins across the session's whole life and
  // across restarts — log into the staging app once, stay logged in.
  let stateFile = null;
  if (params.profile) {
    const dir = path.join(params.artifactDir || ".", "profiles");
    fs.mkdirSync(dir, { recursive: true });
    stateFile = path.join(dir, `${params.profile}-${engine}.json`);
    if (fs.existsSync(stateFile)) contextOpts.storageState = stateFile;
  }

  const context = await browser.newContext(contextOpts);
  const page = await context.newPage();

  const session = {
    id, engine, context, page, stateFile,
    consoleLog: [],
    artifactDir: params.artifactDir || ".",
    snapshotCount: 0,
  };

  page.on("console", (m) => push(session, m.type(), m.text()));
  page.on("pageerror", (e) => push(session, "pageerror", String(e)));
  page.on("requestfailed", (r) => {
    const f = r.failure();
    // Aborted requests are routine (cancelled prefetches, navigations); only a
    // real transport failure is worth an agent's attention.
    if (f && !/ERR_ABORTED/.test(f.errorText)) push(session, "requestfailed", `${r.url()} — ${f.errorText}`);
  });

  sessions.set(id, session);
  return session;
}

function push(session, type, text) {
  session.consoleLog.push({ type, text: String(text).slice(0, 400) });
  if (session.consoleLog.length > CONSOLE_CAP) session.consoleLog.shift();
}

function requireSession(id) {
  const s = sessions.get(id);
  if (!s || s.page.isClosed()) {
    throw new Error(`no open browser for this session — call {"action":"open","url":"…"} first`);
  }
  return s;
}

async function closeSession(id) {
  const s = sessions.get(id);
  if (!s) return false;
  sessions.delete(id);
  if (s.stateFile) { try { await s.context.storageState({ path: s.stateFile }); } catch {} }
  try { await s.context.close(); } catch {}
  return true;
}

async function closeAll() {
  await Promise.all([...sessions.keys()].map((id) => closeSession(id).catch(() => {})));
}

// ── Snapshot: stamp refs, return a compact tree ──────────────────────────────

// Runs inside the page. Returns [{ref, role, name, depth, extra}] for every
// visible element worth acting on, having stamped data-diri-ref on each.
const SNAPSHOT_FN = function (opts) {
  const INTERACTIVE = [
    "a[href]", "button", "input:not([type=hidden])", "select", "textarea", "summary",
    "[role=button]", "[role=link]", "[role=checkbox]", "[role=radio]", "[role=tab]",
    "[role=menuitem]", "[role=switch]", "[role=combobox]", "[role=textbox]",
    "[role=option]", "[role=slider]", "[contenteditable='']", "[contenteditable=true]",
    "[onclick]", "[tabindex]:not([tabindex='-1'])",
  ].join(",");
  const STRUCTURAL = "h1,h2,h3,h4,h5,h6,[role=heading],[role=alert],[role=status],[role=dialog],label,legend,th,caption";

  for (const el of document.querySelectorAll("[data-diri-ref]")) el.removeAttribute("data-diri-ref");

  function visible(el) {
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) return false;
    const st = getComputedStyle(el);
    if (st.visibility === "hidden" || st.display === "none" || st.opacity === "0") return false;
    if (el.closest("[aria-hidden='true']")) return false;
    return true;
  }

  function textOf(el) {
    return (el.innerText || el.textContent || "").replace(/\s+/g, " ").trim();
  }

  // Follows the ARIA accname precedence: aria-label, aria-labelledby, then the
  // native <label>, and only then the placeholder. Getting this order wrong is
  // not cosmetic — the name is the handle a model grabs the element by, and a
  // placeholder ("you@example.com") describes the example, not the field.
  function accName(el) {
    const aria = el.getAttribute("aria-label");
    if (aria) return aria.trim();
    const by = el.getAttribute("aria-labelledby");
    if (by) {
      const parts = by.split(/\s+/).map((i) => document.getElementById(i)).filter(Boolean).map(textOf);
      if (parts.length) return parts.join(" ").trim();
    }
    if (el.tagName === "IMG" && el.alt) return el.alt.trim();

    // Form controls never fall through to descendant text: a <select> would
    // otherwise be named after its concatenated options.
    if (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT") {
      if (el.id) {
        const lbl = document.querySelector(`label[for="${CSS.escape(el.id)}"]`);
        if (lbl) return textOf(lbl).slice(0, 80);
      }
      const wrap = el.closest("label");
      if (wrap) return textOf(wrap).slice(0, 80);
      if (el.placeholder) return el.placeholder.trim();
      if (el.name) return el.name;
      return (el.getAttribute("title") || "").trim();
    }

    const t = textOf(el);
    if (t) return t.slice(0, 80);
    return (el.getAttribute("title") || "").trim();
  }

  function roleOf(el) {
    const explicit = el.getAttribute("role");
    if (explicit) return explicit;
    const tag = el.tagName.toLowerCase();
    if (tag === "a") return "link";
    if (tag === "button" || tag === "summary") return "button";
    if (tag === "select") return "combobox";
    if (tag === "textarea") return "textbox";
    if (/^h[1-6]$/.test(tag)) return "heading";
    if (tag === "input") {
      const t = (el.type || "text").toLowerCase();
      if (t === "checkbox") return "checkbox";
      if (t === "radio") return "radio";
      if (t === "submit" || t === "button" || t === "reset") return "button";
      if (t === "range") return "slider";
      return "textbox";
    }
    return tag;
  }

  const selector = opts.full ? INTERACTIVE + "," + STRUCTURAL : INTERACTIVE;
  const all = [...document.querySelectorAll(selector)].filter(visible);

  // Depth among *matched* elements only, so nesting reflects the interactive
  // structure an agent cares about rather than raw DOM depth.
  const set = new Set(all);
  const out = [];
  let n = 0;
  for (const el of all) {
    let depth = 0;
    for (let p = el.parentElement; p; p = p.parentElement) if (set.has(p)) depth++;
    const ref = "e" + ++n;
    el.setAttribute("data-diri-ref", ref);

    const extra = {};
    const type = (el.type || "").toLowerCase();
    // A checkbox's DOM value is the constant "on" — `checked` is the state that
    // actually varies, so reporting both is pure noise.
    const valueless = type === "checkbox" || type === "radio" || type === "password";
    if (el.value !== undefined && !valueless && String(el.value)) {
      extra.value = String(el.value).slice(0, 60);
    }
    if (el.disabled) extra.disabled = true;
    if (el.checked) extra.checked = true;
    if (el.tagName === "A" && el.getAttribute("href")) {
      extra.href = el.getAttribute("href").slice(0, 100);
    }
    out.push({ ref, role: roleOf(el), name: accName(el), depth: Math.min(depth, 6), extra });
  }
  return { nodes: out, url: location.href, title: document.title };
};

function renderSnapshot(data) {
  const lines = data.nodes.map((n) => {
    const bits = [];
    for (const [k, v] of Object.entries(n.extra)) {
      bits.push(v === true ? k : `${k}=${JSON.stringify(v)}`);
    }
    const name = n.name ? ` ${JSON.stringify(n.name)}` : "";
    return `${"  ".repeat(n.depth)}@${n.ref} ${n.role}${name}${bits.length ? " " + bits.join(" ") : ""}`;
  });
  return lines.join("\n");
}

async function snapshot(session, params) {
  const data = await session.page.evaluate(SNAPSHOT_FN, { full: !!params.full });
  session.snapshotCount++;
  const text = renderSnapshot(data);
  const capped = text.length > 12000 ? text.slice(0, 12000) + "\n… (truncated — pass a selector or use get)" : text;
  return {
    url: data.url,
    title: data.title,
    elements: data.nodes.length,
    snapshot: capped,
    hint: data.nodes.length === 0
      ? "Nothing interactive is visible. The page may still be loading — try {\"action\":\"wait\",\"state\":\"networkidle\"} then snapshot again."
      : undefined,
  };
}

// ── Acting ───────────────────────────────────────────────────────────────────

/// Resolves a ref or CSS selector to a Playwright locator. A ref that no longer
/// resolves is nearly always a navigation, so the error says so instead of
/// leaving the agent to conclude the element "doesn't exist".
async function locate(session, params) {
  if (params.ref) {
    const ref = String(params.ref).replace(/^@/, "");
    const loc = session.page.locator(`[data-diri-ref="${ref}"]`);
    if ((await loc.count()) === 0) {
      throw new Error(
        `@${ref} is no longer on the page — refs go stale when the page navigates or re-renders. Take a fresh snapshot and use the new ref.`);
    }
    return loc.first();
  }
  if (params.selector) return session.page.locator(params.selector).first();
  throw new Error("this action needs a ref (from snapshot) or a selector");
}

async function act(session, params) {
  const page = session.page;
  const a = params.action;

  switch (a) {
    case "click": {
      const loc = await locate(session, params);
      await loc.click({ button: params.button || "left", clickCount: params.double ? 2 : 1, timeout: 8000 });
      return { ok: true };
    }
    case "fill": {
      const loc = await locate(session, params);
      await loc.fill(String(params.text ?? ""), { timeout: 8000 });
      return { ok: true };
    }
    case "type": {
      // Unlike fill, this leaves existing content and emits real keystrokes —
      // the path that works for editors and key-driven UIs.
      if (params.ref || params.selector) {
        const loc = await locate(session, params);
        await loc.pressSequentially(String(params.text ?? ""), { timeout: 8000 });
      } else {
        await page.keyboard.type(String(params.text ?? ""));
      }
      return { ok: true };
    }
    case "press": {
      if (!params.key) throw new Error("press needs a key, e.g. Enter or Control+a");
      if (params.ref || params.selector) {
        const loc = await locate(session, params);
        await loc.press(params.key, { timeout: 8000 });
      } else {
        await page.keyboard.press(params.key);
      }
      return { ok: true };
    }
    case "hover": {
      const loc = await locate(session, params);
      await loc.hover({ timeout: 8000 });
      return { ok: true };
    }
    case "select": {
      const loc = await locate(session, params);
      await loc.selectOption(String(params.value ?? ""), { timeout: 8000 });
      return { ok: true };
    }
    case "check": {
      const loc = await locate(session, params);
      params.value === "false" ? await loc.uncheck({ timeout: 8000 }) : await loc.check({ timeout: 8000 });
      return { ok: true };
    }
    case "scroll": {
      if (params.ref || params.selector) {
        const loc = await locate(session, params);
        await loc.scrollIntoViewIfNeeded({ timeout: 8000 });
        return { ok: true, scrolled: "into view" };
      }
      const amount = params.amount || 600;
      const [dx, dy] = {
        up: [0, -amount], down: [0, amount],
        left: [-amount, 0], right: [amount, 0],
      }[params.direction || "down"] || [0, amount];
      await page.mouse.wheel(dx, dy);
      return { ok: true };
    }
    default:
      throw new Error(`unknown action: ${a}`);
  }
}

// ── Reading ──────────────────────────────────────────────────────────────────

async function get(session, params) {
  const what = params.what || "url";
  const page = session.page;
  switch (what) {
    case "url": return { url: page.url() };
    case "title": return { title: await page.title() };
    case "text": {
      const loc = params.ref || params.selector ? await locate(session, params) : page.locator("body");
      const t = (await loc.innerText()).replace(/\n{3,}/g, "\n\n");
      return { text: t.length > 8000 ? t.slice(0, 8000) + "\n… (truncated)" : t };
    }
    case "html": {
      const loc = params.ref || params.selector ? await locate(session, params) : page.locator("body");
      const h = await loc.innerHTML();
      return { html: h.length > 8000 ? h.slice(0, 8000) + "\n… (truncated)" : h };
    }
    case "value": {
      const loc = await locate(session, params);
      return { value: await loc.inputValue() };
    }
    case "count": {
      if (!params.selector) throw new Error("count needs a selector");
      return { count: await page.locator(params.selector).count() };
    }
    default:
      throw new Error(`unknown get target: ${what} (url|title|text|html|value|count)`);
  }
}

async function waitFor(session, params) {
  const page = session.page;
  if (typeof params.ms === "number") { await page.waitForTimeout(params.ms); return { ok: true, waited: `${params.ms}ms` }; }
  if (params.selector) {
    await page.waitForSelector(params.selector, { timeout: params.timeout || 10000, state: params.state || "visible" });
    return { ok: true, waited: params.selector };
  }
  const state = params.state || "load";
  await page.waitForLoadState(state, { timeout: params.timeout || 15000 });
  return { ok: true, waited: state };
}

// ── Screenshot ───────────────────────────────────────────────────────────────

// Draws a labelled box over every ref so the image and the snapshot text line
// up — without it an agent has to correlate "@e7" against pixels by guesswork.
const ANNOTATE_FN = function () {
  const layer = document.createElement("div");
  layer.id = "__diri_annotations";
  layer.style.cssText = "position:fixed;inset:0;z-index:2147483647;pointer-events:none";
  for (const el of document.querySelectorAll("[data-diri-ref]")) {
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) continue;
    if (r.bottom < 0 || r.top > innerHeight) continue;
    const box = document.createElement("div");
    box.style.cssText = `position:absolute;left:${r.left}px;top:${r.top}px;width:${r.width}px;height:${r.height}px;border:1.5px solid #ff2d55;box-sizing:border-box`;
    const tag = document.createElement("div");
    tag.textContent = "@" + el.getAttribute("data-diri-ref");
    tag.style.cssText = `position:absolute;left:${r.left}px;top:${Math.max(0, r.top - 14)}px;background:#ff2d55;color:#fff;font:10px/1.2 -apple-system,sans-serif;padding:1px 3px;border-radius:2px`;
    layer.appendChild(box);
    layer.appendChild(tag);
  }
  document.body.appendChild(layer);
};

async function screenshot(session, params) {
  const dir = session.artifactDir;
  fs.mkdirSync(dir, { recursive: true });
  const annotated = !!params.annotate;
  if (annotated) await session.page.evaluate(ANNOTATE_FN).catch(() => {});
  const file = path.join(dir, `${session.id}-${session.engine}-${Date.now()}${annotated ? "-annotated" : ""}.jpg`);
  try {
    await session.page.screenshot({ path: file, type: "jpeg", quality: 60, fullPage: !!params.full });
  } finally {
    if (annotated) {
      await session.page.evaluate(() => document.getElementById("__diri_annotations")?.remove()).catch(() => {});
    }
  }
  return { screenshotPath: file, annotated };
}

// ── Dispatch ─────────────────────────────────────────────────────────────────

async function handleBrowser(getBrowser, params) {
  const action = params.action;
  const id = params.sessionId;
  // Traced to stderr, which the daemon folds into its log: an agent-driven
  // browser fails in ways (a hung navigation, a selector that never resolves)
  // that are invisible from the result alone.
  console.error(`[browser] ${id} ${action}${params.url ? " " + params.url : ""}${params.ref ? " @" + params.ref : ""}`);

  if (action === "close") return { ok: await closeSession(id) };
  if (action === "list") {
    return {
      sessions: [...sessions.values()].map((s) => ({
        sessionId: s.id, engine: s.engine, url: s.page.isClosed() ? null : s.page.url(),
      })),
    };
  }

  if (action === "open") {
    const session = await ensureSession(getBrowser, params);
    if (params.url) {
      await session.page.goto(params.url, { waitUntil: "domcontentloaded", timeout: 30000 });
    }
    const snap = await snapshot(session, params);
    return { ...snap, engine: session.engine };
  }

  const session = requireSession(id);
  switch (action) {
    case "snapshot": return snapshot(session, params);
    case "screenshot": return screenshot(session, params);
    case "wait": return waitFor(session, params);
    case "get": return get(session, params);
    case "console": {
      const log = session.consoleLog.slice(-(params.amount || 50));
      return { console: log, total: session.consoleLog.length };
    }
    case "back": {
      await session.page.goBack({ waitUntil: "domcontentloaded" });
      return snapshot(session, params);
    }
    default: {
      const result = await act(session, params);
      // Every mutation invalidates the refs it just used, so hand back a fresh
      // snapshot in the same call: it saves a round trip and makes acting on a
      // stale ref structurally hard rather than merely discouraged.
      const snap = await snapshot(session, params);
      return { ...result, ...snap };
    }
  }
}

module.exports = { handleBrowser, closeAll, openSessionCount: () => sessions.size };
