import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { releaseDownloads } from '../downloads.js';
import { buildDocs, pageUrl } from './docs.mjs';
import { render } from './render.mjs';
import { buildSite } from './site.mjs';
import { base, root } from './render.mjs';

test('nested playbook steps stay inside one ordered list', () => {
  const { html } = render('1. Before you leave:\n\n   - Name every live session.\n   - Give each writer its own worktree.\n\n2. Archive what you merged.\n');
  assert.match(html, /<ol><li>Before you leave:<ul><li>Name every live session\.<\/li><li>Give each writer its own worktree\.<\/li><\/ul><\/li><li>Archive what you merged\.<\/li><\/ol>/);
});

test('wrapped list items stay one item', () => {
  const { html } = render('1. Choose the long option that\n   wraps onto the next line.\n');
  assert.match(html, /<li>Choose the long option that wraps onto the next line\.<\/li>/);
});

test('user docs and playbooks share /docs and developer chapters are not published', async () => {
  const { outputs, urls } = await buildDocs();
  assert.equal(pageUrl('introduction'), '/docs/');
  assert.equal(pageUrl('playbooks'), '/docs/playbooks/');
  assert.equal(pageUrl('playbooks/parallel-worktrees'), '/docs/playbooks/parallel-worktrees/');
  assert.ok(urls.includes('/docs/playbooks/'));
  assert.ok(urls.every((url) => url.startsWith('/docs/')));
  assert.ok(!urls.some((url) => url.startsWith('/docs/dev') || url.startsWith('/guides') || url.startsWith('/playbooks')));
  assert.equal(outputs.has('docs/index.html'), true);
  assert.equal(outputs.has('docs/playbooks/index.html'), true);
  assert.equal(outputs.has('docs/playbooks/parallel-worktrees/index.html'), true);
  assert.equal(outputs.has('docs/dev/index.html'), true);
  assert.equal([...outputs.keys()].some((file) => file.startsWith('docs/dev/') && file !== 'docs/dev/index.html'), false);
  assert.match(outputs.get('docs/dev/index.html'), /github\.com\/nnayz\/taskmaster\/blob\/main\/docs\/dev\/src\/introduction\.md/);
  assert.match(outputs.get('docs/playbooks/idle-and-limits/index.html'), /<ul><li>Name every live session\./);
  assert.match(outputs.get('docs/getting-started.html'), /url=getting-started\//);
  const devPointer = outputs.get('docs/dev/index.html');
  assert.doesNotMatch(devPointer, /Holder internals/);
});

test('generated site links resolve and do not publish a second docs host', async () => {
  const outputs = await buildSite();
  assert.equal(outputs.has('index.html'), true);
  assert.equal(outputs.has('install/index.html'), true);
  assert.equal(outputs.has('llms-full.txt'), true);
  assert.match(outputs.get('llms.txt'), /\/docs\//);
  assert.match(outputs.get('llms-full.txt'), /playbooks/);
  assert.doesNotMatch(outputs.get('llms-full.txt'), /This is the Developer Guide/);
  const html = [...outputs.entries()].filter(([file]) => file.endsWith('.html')).map(([, content]) => content).join('\n');
  assert.equal(html.includes('fumadocs') || html.includes('mdbook') || html.includes('diri.sh'), false);
  for (const content of outputs.values()) {
    for (const match of content.matchAll(/(?:href|src)="([^"]+)"/g)) {
      const url = match[1].split('#')[0].split('?')[0];
      if (!url.startsWith('/') || url.startsWith('//')) continue;
      assert.equal(await localExists(url, outputs), true, url);
    }
  }
});

async function localExists(url, outputs) {
  // GitHub Pages serves the site under a base path; strip it so the
  // deployment-prefixed URLs resolve against the base-path-free outputs.
  if (base && (url === base || url.startsWith(`${base}/`))) url = url.slice(base.length) || '/';
  if (url === '/') return outputs.has('index.html');
  if (url.endsWith('/')) return outputs.has(`${url.slice(1)}index.html`);
  if (outputs.has(url.slice(1))) return true;
  const disk = [
    resolve(root, url.slice(1)),
    resolve(root, 'static', url.slice(1)),
    resolve(root, '..', 'docs/src', url.replace(/^\/docs\//, '')),
  ];
  for (const file of disk) {
    try { await readFile(file); return true; } catch {}
  }
  return false;
}

test('latest-release parsing keeps only a finished macOS DMG and ZIP', () => {
  const release = {
    draft: false,
    prerelease: false,
    tag_name: 'v0.3.0',
    assets: [
      asset('taskmaster-0.3.0-universal.dmg'),
      asset('taskmaster-0.3.0-universal.zip'),
      asset('taskmaster-0.3.0-linux.AppImage'),
      { name: 'taskmaster-0.3.0-universal.dmg', state: 'uploaded', size: 10, browser_download_url: 'https://example.test/wrong' },
    ],
  };
  const parsed = releaseDownloads(release);
  assert.deepEqual(parsed.downloads.map((item) => item.name), ['taskmaster-0.3.0-universal.dmg', 'taskmaster-0.3.0-universal.zip']);
  assert.equal(releaseDownloads({ ...release, draft: true }), null);
  assert.equal(releaseDownloads({ ...release, prerelease: true }), null);
  assert.equal(releaseDownloads({ ...release, tag_name: 'nightly' }), null);
});

function asset(name) {
  return {
    name,
    state: 'uploaded',
    size: 100,
    browser_download_url: `https://github.com/nnayz/taskmaster/releases/download/v0.3.0/${name}`,
  };
}
