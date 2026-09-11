import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { createFixture } from '../dev-server.mjs';
import { launchBrowser } from './cdp.mjs';

const executable = process.env.COMPANION_BROWSER;
const click = selector => `document.querySelector(${JSON.stringify(selector)}).click()`;
const value = (selector, text) => `document.querySelector(${JSON.stringify(selector)}).value = ${JSON.stringify(text)}`;
async function setup(t) {
  const fixture = await createFixture(); t.after(() => fixture.close());
  const browser = await launchBrowser(executable); t.after(() => browser.close());
  await browser.viewport(390, 844); await browser.navigate(fixture.origin);
  return { fixture, browser };
}
async function pair(browser, fixture) {
  await browser.evaluate(value('#payload', JSON.stringify(fixture.issuePairing())));
  await browser.evaluate(value('#device-name', 'Browser fixture phone'));
  await browser.evaluate('document.querySelector("#pair-form").requestSubmit()');
  await browser.wait('!document.querySelector("#identity").hidden');
  assert.equal(await browser.evaluate('document.querySelector("#payload").value'), '');
  assert.equal(fixture.stats.pairings, 0, 'identity review precedes sending the pairing code');
  await browser.evaluate(click('#confirm-pair'));
  await browser.wait('document.querySelector("#connection").textContent === "Connected"');
  await browser.wait('document.querySelectorAll(".session-row").length === 2');
}
async function screenshot(browser, name) {
  if (!process.env.COMPANION_SCREENSHOT_DIR) return;
  await mkdir(process.env.COMPANION_SCREENSHOT_DIR, { recursive: true });
  const { data } = await browser.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  await writeFile(join(process.env.COMPANION_SCREENSHOT_DIR, name + '.png'), Buffer.from(data, 'base64'));
}

test('mobile browser: pair, inspect, explicit control, prompt, lifecycle cancel, privacy and revoke', { skip: !executable }, async t => {
  const { browser, fixture } = await setup(t);
  await screenshot(browser, 'pairing');
  await pair(browser, fixture);
  await browser.evaluate(click('[data-session="session_remote"]'));
  await browser.wait('document.querySelector("#screen").textContent.includes("Migration ready")');
  assert.equal(await browser.evaluate('document.querySelector("#send-prompt").disabled'), true);
  assert.equal(fixture.stats.acquisitions, 0);
  await screenshot(browser, 'session');
  await browser.evaluate(click('#take-control')); await browser.wait('document.querySelector("#confirmation").open');
  assert.equal(await browser.evaluate('document.activeElement.id'), 'cancel-action');
  await browser.evaluate(click('#confirm-action'));
  await browser.wait('!document.querySelector("#send-prompt").disabled');
  await browser.evaluate(value('#prompt', 'Synthetic browser prompt.'));
  await browser.evaluate('document.querySelector("#prompt-form").requestSubmit()');
  await browser.wait('document.querySelector("#screen").textContent.includes("Prompt received")');
  assert.equal(fixture.stats.prompts, 1);
  assert.equal(await browser.evaluate('document.querySelector("#prompt").value'), '');
  await browser.wait('!document.querySelector("#actions button").disabled');
  await browser.evaluate('document.querySelector(".lifecycle").open = true; [...document.querySelectorAll("#actions button")].find(button => button.textContent === "Terminate").click()');
  await browser.wait('document.querySelector("#confirmation").open');
  await browser.evaluate(click('#cancel-action')); assert.equal(fixture.stats.actions, 0);
  await browser.evaluate('[...document.querySelectorAll("#actions button")].find(button => button.textContent === "Rename").click()');
  await browser.evaluate(value('#rename', 'Reviewed browser fixture'));
  await browser.evaluate(click('#confirm-action'));
  await browser.wait('document.querySelector("#session-title").textContent === "Reviewed browser fixture"');
  assert.equal(fixture.stats.actions, 1);
  assert.deepEqual(await browser.evaluate('({local:localStorage.length,session:sessionStorage.length,cookies:document.cookie})'), { local: 0, session: 0, cookies: '' });
  await browser.evaluate(click('#privacy'));
  await browser.wait('!document.querySelector("#privacy-cover").hidden');
  assert.equal(await browser.evaluate('document.querySelector("#screen").textContent'), '');
  assert.equal(await browser.evaluate('document.querySelector("#projects").textContent'), '');
  await screenshot(browser, 'privacy');
  const requests = fixture.stats.requests;
  await new Promise(resolve => setTimeout(resolve, 350));
  assert.equal(fixture.stats.requests, requests, 'hidden client performs no polling or subscription work');
  await browser.evaluate(click('#reveal')); await browser.wait('document.querySelector("#connection").textContent === "Connected"');
  assert.equal(fixture.stats.prompts, 1);
  fixture.setMode('desktop');
  await browser.wait('document.querySelector("#controller").textContent.includes("Zeus desktop")');
  assert.equal(await browser.evaluate('document.querySelector("#send-prompt").disabled'), true);
  fixture.setMode('revoked');
  await browser.wait('document.querySelector("#connection").textContent === "Device revoked"');
  assert.equal(await browser.evaluate('document.querySelector("#screen").textContent'), '');
  assert.equal(await browser.evaluate('document.querySelector("#workspace").hidden'), true);
  t.diagnostic(`Validated ${browser.version} with synthetic data; no real iOS privacy claim.`);
});

test('mobile browser: portrait, landscape, large text, accessible controls and inert terminal output', { skip: !executable }, async t => {
  const { browser, fixture } = await setup(t); await pair(browser, fixture);
  await browser.evaluate(click('[data-session="session_local"]')); await browser.wait('document.querySelector("#screen").textContent.includes("Companion prototype")');
  for (const [width, height] of [[375, 812], [812, 375], [320, 568]]) {
    await browser.viewport(width, height);
    assert.equal(await browser.evaluate('document.documentElement.scrollWidth <= innerWidth'), true, `${width}×${height} avoids horizontal page overflow`);
  }
  await browser.viewport(390, 844);
  await browser.evaluate('document.documentElement.style.fontSize = "200%"');
  assert.equal(await browser.evaluate('document.documentElement.scrollWidth <= innerWidth'), true, 'large text reflows');
  await browser.evaluate('document.documentElement.style.fontSize = "100%"');
  const smallControls = await browser.evaluate('[...document.querySelectorAll("button,input,summary")].filter(element=>element.getClientRects().length && !element.closest("[hidden]" )).filter(element=>element.getBoundingClientRect().height<44).map(element=>element.id || element.tagName)');
  assert.deepEqual(smallControls, []);
  const unlabelled = await browser.evaluate('[...document.querySelectorAll("textarea,input")].filter(element=>!document.querySelector(`label[for="${element.id}"]`)).map(element=>element.id)');
  assert.deepEqual(unlabelled, []);
  await browser.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
  assert.equal(await browser.evaluate('matchMedia("(prefers-reduced-motion: reduce)").matches'), true);
  assert.equal(await browser.evaluate('document.querySelector("#screen").hasAttribute("aria-live")'), false);
  fixture.setScreenText('session_local', '<img src=x onerror="window.terminalInjected=true">\nUntrusted \u202eoutput');
  await browser.wait('document.querySelector("#screen").textContent.includes("<img")');
  assert.equal(await browser.evaluate('document.querySelector("#screen").childElementCount'), 0);
  assert.equal(await browser.evaluate('window.terminalInjected === undefined'), true);
  assert.equal(await browser.evaluate('document.querySelector("#screen").textContent.includes("\u202e")'), false);
  const response = await browser.evaluate('fetch("/v1/sessions").then(response=>({status:response.status,cache:response.headers.get("cache-control")}))');
  assert.equal(response.status, 401); assert.equal(response.cache, 'no-store');
  const cachedURLs = await browser.evaluate('caches.keys().then(async keys => (await Promise.all(keys.map(async key => (await (await caches.open(key)).keys()).map(request=>new URL(request.url).pathname)))).flat())');
  assert.ok(cachedURLs.every(path => !path.startsWith('/v1') && !path.startsWith('/fixture')));
  await browser.send('Page.reload'); await browser.wait('document.querySelector("#connection")?.textContent === "Not paired"');
  assert.equal(await browser.evaluate('document.querySelector("#workspace").hidden'), true, 'reload has no retained device credential');
});

test('mobile browser: lost delivery stays blocked until explicit review, offline unpair still deletes data', { skip: !executable }, async t => {
  const { browser, fixture } = await setup(t); await pair(browser, fixture);
  await browser.evaluate(click('[data-session="session_local"]')); await browser.wait('document.querySelector("#screen").textContent.includes("Companion prototype")');
  await browser.evaluate(click('#take-control')); await browser.evaluate(click('#confirm-action'));
  await browser.wait('!document.querySelector("#send-prompt").disabled');
  fixture.setMode('drop_mutation'); await browser.evaluate(value('#prompt', 'Exactly one browser prompt.')); await browser.evaluate('document.querySelector("#prompt-form").requestSubmit()');
  await browser.wait('document.querySelector("#connection").textContent.includes("Offline")'); assert.equal(fixture.stats.prompts, 1);
  fixture.setMode('online'); await browser.evaluate(click('#refresh')); await browser.wait('document.querySelector("#connection").textContent === "Connected"');
  assert.equal(await browser.evaluate('document.querySelector("#send-prompt").disabled'), true);
  assert.equal(await browser.evaluate('document.querySelector("#review-delivery").hidden'), false);
  await browser.evaluate(click('#review-delivery')); await browser.wait('!document.querySelector("#send-prompt").disabled');
  assert.equal(fixture.stats.prompts, 1);
  fixture.setMode('offline'); await browser.wait('document.querySelector("#connection").textContent.includes("Offline")');
  await browser.evaluate(click('#unpair')); await browser.evaluate(click('#confirm-action'));
  try { await browser.wait('document.querySelector("#connection").textContent === "Not paired"', 12000); }
  catch (error) { t.diagnostic(JSON.stringify(await browser.evaluate('({connection:document.querySelector("#connection").textContent,notice:document.querySelector("#notice").textContent,dialog:document.querySelector("#confirmation").open,workspaceHidden:document.querySelector("#workspace").hidden})'))); throw error; }
  assert.equal(await browser.evaluate('document.querySelector("#screen").textContent'), '');
  assert.ok(await browser.evaluate('document.querySelector("#notice").textContent.includes("revocation could not be confirmed")'));
});
