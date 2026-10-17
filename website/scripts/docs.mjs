// Builds /docs from docs/src, including playbooks under the same /docs base.
// Developer pages in docs/dev are not published.
import { readFile, readdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { abs, base, devRepo, docsRepo, esc, inline, plain, render, rootPath, shell, slugify, splitPage, header } from './render.mjs';
import { repoRoot } from './render.mjs';

const source = resolve(repoRoot, 'docs/src');

const ICONS = {
  introduction: 'file',
  'getting-started': 'download',
  workbench: 'sidebar',
  keyboard: 'keyboard',
  agents: 'apps',
  worktrees: 'branch',
  orchestration: 'plug',
  fleet: 'local-agents',
  settings: 'settings',
  cli: 'terminal',
  'remote-hosts': 'server',
  'remote-nodes': 'server',
  'ios-companion': 'monitor',
  updates: 'refresh',
  playbooks: 'note',
  'playbooks/parallel-worktrees': 'branch',
  'playbooks/delegate': 'new-agent',
  'playbooks/review-pull-request': 'pull-request',
  'playbooks/local-or-remote': 'server',
  'playbooks/needs-input': 'warning',
  'playbooks/idle-and-limits': 'moon',
  'security-model': 'shield',
  'security-policy': 'shield',
  privacy: 'shield',
  support: 'lifebuoy',
  roadmap: 'chevron-right',
};

export function parseSummary(text) {
  const groups = [];
  let current = null;
  for (const line of text.split('\n')) {
    const heading = line.match(/^# (.+)$/);
    if (heading) {
      if (heading[1] === 'Summary') continue;
      current = { group: heading[1], pages: [] };
      groups.push(current);
      continue;
    }
    const link = line.match(/\[([^\]]+)\]\(([^)]+\.md)\)/);
    if (!link) continue;
    if (!current) {
      current = { group: 'Overview', pages: [] };
      groups.push(current);
    }
    const file = link[2];
    let slug = file.replace(/\.md$/, '');
    if (slug.endsWith('/index')) slug = slug.slice(0, -'/index'.length);
    current.pages.push({ slug, nav: link[1], file });
  }
  return groups;
}

export function pageUrl(slug) {
  return slug === 'introduction' ? '/docs/' : `/docs/${slug}/`;
}

export function rewriteDocHref(fromFile, href) {
  const hashAt = href.indexOf('#');
  const hash = hashAt >= 0 ? href.slice(hashAt) : '';
  const raw = hashAt >= 0 ? href.slice(0, hashAt) : href;
  if (!raw || /^(https?:|mailto:)/.test(raw)) return href;
  if (raw.startsWith('dev/')) {
    const page = raw.replace(/^dev\//, '').replace(/\.html$/, '');
    return `${devRepo}/${page}.md${hash}`;
  }
  const dir = fromFile.includes('/') ? fromFile.slice(0, fromFile.lastIndexOf('/')) : '';
  const parts = [];
  for (const part of `${dir}/${raw}`.split('/')) {
    if (!part || part === '.') continue;
    if (part === '..') parts.pop();
    else parts.push(decodeURIComponent(part));
  }
  const joined = parts.join('/');
  if (/\.(svg|png|webp|gif|jpe?g)$/i.test(joined)) return rootPath(`/docs/${joined}${hash}`);
  let slug = joined.replace(/\.md$/, '').replace(/\.html$/, '');
  if (slug.endsWith('/index')) slug = slug.slice(0, -'/index'.length);
  if (slug === 'introduction' || slug === 'index' || slug === '') return rootPath(`/docs/${hash}`);
  return rootPath(`${pageUrl(slug)}${hash}`);
}

async function markdownFiles(dir, prefix = '') {
  const found = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    if (entry.name.startsWith('.')) continue;
    const rel = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) found.push(...await markdownFiles(resolve(dir, entry.name), rel));
    else if (entry.name.endsWith('.md') && entry.name !== 'SUMMARY.md') found.push(rel);
  }
  return found;
}

async function imageFiles(dir, prefix = 'images') {
  const root = resolve(dir, 'images');
  const found = new Set();
  const walk = async (current, rel) => {
    let entries = [];
    try { entries = await readdir(current, { withFileTypes: true }); } catch { return; }
    for (const entry of entries) {
      const next = rel ? `${rel}/${entry.name}` : entry.name;
      if (entry.isDirectory()) await walk(resolve(current, entry.name), next);
      else found.add(`images/${next}`.replace(/^images\/images\//, 'images/'));
    }
  };
  await walk(root, '');
  return found;
}

function sidebar(current, groups, pages) {
  const html = groups.map(({ group, pages: list }) => `<div class="docs-group"><div class="docs-group-head"><span class="icon" data-icon="chevron-down" aria-hidden="true"></span>${esc(group)}</div><ul>${list.map(({ slug }) => {
    const page = pages.get(slug);
    const icon = ICONS[slug] || 'file';
    return `<li><a class="docs-row" href="${rootPath(pageUrl(slug))}"${slug === current ? ' aria-current="page"' : ''}><span class="icon" data-icon="${icon}" aria-hidden="true"></span><span>${esc(page.nav)}</span></a></li>`;
  }).join('')}</ul></div>`).join('');
  return `<nav class="docs-sidebar" id="docs-sidebar" aria-label="Documentation">
          <button class="docs-search" type="button" data-open-search><span class="icon" data-icon="search" aria-hidden="true"></span><span>Search docs</span><kbd>⌘K</kbd></button>
          ${html}
          <div class="docs-sidebar-foot"><a href="${rootPath('/llms.txt')}"><span class="icon" data-icon="markdown" aria-hidden="true"></span>llms.txt</a><a href="${devRepo}/introduction.md" rel="noopener"><span class="icon" data-icon="github" aria-hidden="true"></span>Developer docs</a></div>
        </nav>`;
}

function template({ slug, page, html, toc, group, prev, next, groups, pages }) {
  const path = pageUrl(slug);
  const raw = slug === 'introduction' ? '/docs/index.md' : `/docs/${slug}.md`;
  const title = slug === 'introduction' ? 'Documentation · Taskmaster' : `${page.title} · Taskmaster docs`;
  const active = slug === 'playbooks' || slug.startsWith('playbooks/') ? 'Guides' : 'Docs';
  const tocHtml = toc.length > 1 ? `<span>On this page</span>${toc.map((item) => `<a href="#${item.id}">${esc(item.text)}</a>`).join('')}` : '';
  const pager = `<nav class="docs-pager" aria-label="Previous and next">${prev ? `<a class="prev" href="${rootPath(pageUrl(prev.slug))}"><span>Previous</span>${esc(prev.title)}</a>` : '<span></span>'}${next ? `<a class="next" href="${rootPath(pageUrl(next.slug))}"><span>Next</span>${esc(next.title)}</a>` : '<span></span>'}</nav>`;
  const body = `${header(active)}
    <div class="docs-shell">
      <button class="docs-menu" type="button" aria-expanded="false" aria-controls="docs-sidebar"><span class="icon" data-icon="sidebar" aria-hidden="true"></span>${esc(group)}<span class="docs-menu-sep">/</span><strong>${esc(page.nav)}</strong></button>
      ${sidebar(slug, groups, pages)}
      <main id="main" class="docs-main" data-raw="${rootPath(raw)}">
        <header class="docs-heading"><span class="eyebrow">${esc(group)}</span><h1>${esc(page.title)}</h1><p>${inline(page.lead)}</p></header>
        <article class="docs-content">
${html}
        </article>
        ${pager}
      </main>
      <aside class="docs-rail" aria-label="On this page">
        <div class="docs-rail-inner">${tocHtml ? `<nav class="docs-toc">${tocHtml}</nav>` : ''}
          <div class="docs-actions"><button type="button" data-copy-page><span class="icon" data-icon="copy" aria-hidden="true"></span><span>Copy as Markdown</span></button><a href="${rootPath(raw)}"><span class="icon" data-icon="markdown" aria-hidden="true"></span>View Markdown</a><a href="${docsRepo}/${page.file}" rel="noopener"><span class="icon" data-icon="github" aria-hidden="true"></span>Edit on GitHub</a></div>
        </div>
      </aside>
    </div>`;
  return shell({
    title,
    description: page.description,
    path,
    body,
    scripts: `<script src="${rootPath('/docs.js')}" defer></script>`,
  }).replace('<body>', '<body class="guide-page docs-page">');
}

function redirect(fromFile, target) {
  const external = /^https?:/.test(target);
  const canonical = external ? target : abs(target);
  const href = external ? target : relativeRedirect(fromFile, target);
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <link rel="canonical" href="${esc(canonical)}">
  <meta http-equiv="refresh" content="0; url=${esc(href)}">
  <title>Redirect</title>
  <script>location.replace(${JSON.stringify(href)})</script>
</head>
<body><p><a href="${esc(href)}">Continue</a></p></body>
</html>
`;
}

function relativeRedirect(fromFile, target) {
  const fromDir = fromFile.split('/').slice(0, -1);
  const toParts = target.split('/').filter(Boolean);
  let shared = 0;
  while (shared < fromDir.length && shared < toParts.length && fromDir[shared] === toParts[shared]) shared += 1;
  const up = fromDir.slice(shared).map(() => '..');
  const down = toParts.slice(shared);
  const rel = [...up, ...down].join('/');
  return `${rel || '.'}${target.endsWith('/') ? '/' : ''}`;
}

export async function buildDocs() {
  const groups = parseSummary(await readFile(resolve(source, 'SUMMARY.md'), 'utf8'));
  const order = groups.flatMap(({ group, pages }) => pages.map((page) => ({ ...page, group })));
  const files = await markdownFiles(source);
  const expected = new Set(order.map((page) => page.file));
  for (const file of files) if (!expected.has(file)) throw new Error(`docs/src/${file} is not in SUMMARY.md`);
  for (const page of order) if (!files.includes(page.file)) throw new Error(`SUMMARY.md links to missing ${page.file}`);
  const slugs = new Set(order.map((page) => page.slug));
  const images = await imageFiles(source);
  const pages = new Map();
  for (const page of order) {
    const markdown = await readFile(resolve(source, page.file), 'utf8');
    const split = splitPage(markdown, page.file);
    pages.set(page.slug, { ...page, ...split });
  }

  const outputs = new Map();
  const search = [];
  const llms = ['## Documentation', ''];
  let full = '';
  let lastGroup = '';
  for (const [index, page] of order.entries()) {
    const current = pages.get(page.slug);
    const checked = (href) => {
      const url = rewriteDocHref(page.file, href);
      const local = baseLess(url).split('#')[0].split('?')[0];
      if (local.startsWith('/docs/images/')) {
        const asset = local.slice('/docs/'.length);
        if (!images.has(asset)) throw new Error(`${page.file}: missing image ${asset}`);
      } else if (local.startsWith('/docs/') && !/\.[a-z0-9]+$/i.test(local)) {
        const slug = local.slice('/docs/'.length).replace(/\/$/, '');
        const key = slug || 'introduction';
        if (!slugs.has(key)) throw new Error(`${page.file}: unknown documentation link ${href} -> ${key}`);
      }
      return url;
    };
    const rendered = render(current.body, checked);
    const prev = order[index - 1] ? pages.get(order[index - 1].slug) : null;
    const next = order[index + 1] ? pages.get(order[index + 1].slug) : null;
    const htmlPath = page.slug === 'introduction' ? 'docs/index.html' : `docs/${page.slug}/index.html`;
    outputs.set(htmlPath, template({ slug: page.slug, page: current, html: rendered.html, toc: rendered.toc, group: page.group, prev, next, groups, pages }));
    const raw = `# ${current.title}\n\n${current.body}`;
    outputs.set(page.slug === 'introduction' ? 'docs/index.md' : `docs/${page.slug}.md`, raw);
    if (page.slug !== 'introduction' && !page.slug.endsWith('/index')) {
      const htmlFile = `docs/${page.slug}.html`;
      outputs.set(htmlFile, redirect(htmlFile, pageUrl(page.slug)));
    }
    if (page.group !== lastGroup) {
      llms.push(`\n### ${page.group}\n`);
      lastGroup = page.group;
    }
    llms.push(`- [${current.title}](${abs(page.slug === 'introduction' ? '/docs/index.md' : `/docs/${page.slug}.md`)}): ${current.description}`);
    full += `\n\n---\n\nSource: ${abs(pageUrl(page.slug))}\n\n${raw}`;
    search.push({ t: current.title, g: page.group, u: rootPath(pageUrl(page.slug)), x: current.description });
    for (const section of current.body.split(/^(?=#{2,3} )/m)) {
      const head = section.match(/^(#{2,3}) (.+)$/m);
      if (!head || section.indexOf(head[0]) !== 0) continue;
      const excerpt = plain(section.slice(head[0].length)).slice(0, 220);
      search.push({ t: plain(head[2]), p: current.title, g: page.group, u: `${rootPath(pageUrl(page.slug))}#${slugify(head[2])}`, x: excerpt });
    }
  }
  outputs.set('docs/introduction.html', redirect('docs/introduction.html', '/docs/'));
  outputs.set('docs/companion-architecture.html', redirect('docs/companion-architecture.html', `${devRepo}/companion-architecture.md`));
  outputs.set('docs/screenshot-soak.html', redirect('docs/screenshot-soak.html', `${devRepo}/testing.md#screenshot-soak-test`));
  outputs.set('docs/search.json', `${JSON.stringify(search)}\n`);
  outputs.set('docs/dev/index.html', devPointer());
  return { outputs, order, pages, llms: llms.join('\n'), full, urls: order.map((page) => pageUrl(page.slug)) };
}

function baseLess(url) {
  if (!url.startsWith('/')) return '';
  if (base && (url === base || url.startsWith(`${base}/`))) return url.slice(base.length) || '/';
  return url;
}

function devPointer() {
  const body = `${header('Docs')}
    <main id="main" class="docs-main">
      <header class="docs-heading"><span class="eyebrow">GitHub</span><h1>Developer docs stay on GitHub</h1><p>Building, testing, and architecture notes are not part of this site. User docs and playbooks are both under <a href="${rootPath('/docs/')}">/docs/</a>.</p></header>
      <article class="docs-content"><p><a href="${devRepo}/introduction.md" rel="noopener">Read the developer guide on GitHub</a>.</p></article>
    </main>`;
  return shell({
    title: 'Developer docs · Taskmaster',
    description: 'Taskmaster developer documentation stays in the GitHub repository and is not published on the website.',
    path: '/docs/dev/',
    body,
  }).replace('<body>', '<body class="guide-page docs-page">');
}
