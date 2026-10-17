import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { assetUrls } from '../downloads.js';
import { base, esc, header, render, root, rootPath, shell, splitPage } from './render.mjs';

const pagesDir = resolve(root, 'pages');

function applyTokens(text, release) {
  return text
    .replaceAll('{{version}}', release.version)
    .replaceAll('{{dmg}}', release.dmg)
    .replaceAll('{{zip}}', release.zip)
    .replaceAll('{{github}}', release.github)
    .replaceAll('{{notes}}', release.notes)
    .replaceAll('{{releases}}', release.releases)
    .replaceAll('{{base}}', base);
}

function article({ path, title, description, eyebrow, active, html, toc }) {
  const tocHtml = toc.length > 1
    ? `<nav class="guide-toc" aria-label="On this page"><span>On this page</span>${toc.map((item) => `<a href="#${item.id}">${esc(item.text)}</a>`).join('')}</nav>`
    : '<span></span>';
  const body = `${header(active)}
    <main id="main">
      <header class="guide-heading"><span class="eyebrow">${esc(eyebrow)}</span><h1>${esc(title)}</h1><p>${esc(description)}</p></header>
      <div class="guide-layout">${tocHtml}
        <article class="docs-content article-content">
${html}
        </article>
      </div>
    </main>`;
  return shell({
    title: `${title} · Taskmaster`,
    description,
    path,
    body,
    scripts: `<script src="${rootPath('/docs.js')}" defer></script>`,
  }).replace('<body>', '<body class="guide-page article-page">');
}

function renderMarkdown(markdown) {
  const page = splitPage(markdown, 'page');
  const rendered = render(page.body, (href) => href.startsWith('/') ? rootPath(href) : href);
  return { page, ...rendered };
}

function releaseMarkdown(release, { heading = true } = {}) {
  const paragraphs = release.summary.map((paragraph) => paragraph.trim()).join('\n\n');
  return `${heading ? `# Taskmaster ${release.version}\n\n` : ''}*${release.displayDate}.* This build is ad-hoc signed and not notarized.\n\n${paragraphs}\n\n## Install\n\nDownload the DMG, drag Taskmaster to Applications, then right-click and choose Open. macOS 15 or newer.\n\n- [Download DMG](${release.dmg})\n- [Download ZIP](${release.zip})\n- [GitHub release](${release.github})\n- [Illustrated install guide](/install/)\n- [Security notes](/security/)\n`;
}

export async function buildPages(catalog) {
  const latest = catalog[0];
  const outputs = new Map();
  const home = applyTokens(await readFile(resolve(pagesDir, 'home.html'), 'utf8'), latest);
  outputs.set('index.html', shell({
    title: 'Taskmaster — Run coding agents side by side',
    description: 'Free, open-source Mac app to run Claude Code, Codex, Cursor, and the rest of the catalog side by side. Sessions keep running when you close the window.',
    path: '/',
    sky: true,
    body: `${header('')}\n${home}`,
    scripts: `<script src="${rootPath('/agent-previews.js')}" defer></script><script src="${rootPath('/app.js')}" defer></script><script type="module" src="${rootPath('/downloads.js')}"></script>`,
  }));

  const install = renderMarkdown(applyTokens(await readFile(resolve(pagesDir, 'install.md'), 'utf8'), latest));
  outputs.set('install/index.html', article({
    path: '/install/',
    title: install.page.title,
    description: install.page.description,
    eyebrow: 'Install',
    active: 'Install',
    html: install.html,
    toc: install.toc,
  }));

  const security = renderMarkdown(applyTokens(await readFile(resolve(pagesDir, 'security.md'), 'utf8'), latest));
  outputs.set('security/index.html', article({
    path: '/security/',
    title: security.page.title,
    description: security.page.description,
    eyebrow: 'Security',
    active: '',
    html: security.html,
    toc: security.toc,
  }));

  const notes = catalog.map((release) => `## ${release.version}\n\n${release.displayDate}. ${release.summary[0]}\n\n[Read the notes](/releases/${release.version}/) · [DMG](${release.dmg}) · [GitHub](${release.github})\n`).join('\n');
  const releaseIndex = renderMarkdown(`# Releases\n\nEvery public build so far. Current builds are ad-hoc signed.\n\n${notes}`);
  outputs.set('releases/index.html', article({
    path: '/releases/',
    title: 'Releases',
    description: releaseIndex.page.description,
    eyebrow: 'Releases',
    active: '',
    html: releaseIndex.html,
    toc: releaseIndex.toc,
  }));
  for (const release of catalog) {
    const page = renderMarkdown(releaseMarkdown(release));
    outputs.set(`releases/${release.version}/index.html`, article({
      path: `/releases/${release.version}/`,
      title: page.page.title,
      description: page.page.description,
      eyebrow: 'Release',
      active: '',
      html: page.html,
      toc: page.toc,
    }));
  }
  return { outputs, urls: ['/', '/install/', '/security/', '/releases/', ...catalog.map((release) => `/releases/${release.version}/`)] };
}

export async function loadReleases() {
  const catalog = JSON.parse(await readFile(resolve(root, 'releases.json'), 'utf8'));
  return catalog.map((release) => {
    const [year, month, day] = release.date.split('-').map(Number);
    const displayDate = new Intl.DateTimeFormat('en-US', { month: 'short', day: 'numeric', year: 'numeric', timeZone: 'UTC' }).format(new Date(Date.UTC(year, month - 1, day)));
    return { ...release, ...assetUrls(release.version), displayDate };
  });
}
