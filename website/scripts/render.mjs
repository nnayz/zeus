// Shared Markdown renderer and site chrome.
// Adapted from the Diri website (https://github.com/cristicretu/diri, Apache-2.0).
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const repoRoot = resolve(root, '..');
export const base = (process.env.BASE_PATH || '').replace(/\/$/, '');
export const origin = (process.env.SITE_ORIGIN || 'http://127.0.0.1:4177').replace(/\/$/, '');
export const repo = 'https://github.com/nnayz/taskmaster';
export const docsRepo = `${repo}/blob/main/docs/src`;
export const devRepo = `${repo}/blob/main/docs/dev/src`;

export const abs = (path) => `${origin}${rootPath(path)}`;
export function rootPath(path) {
  if (!path.startsWith('/')) return path;
  if (base && (path === base || path.startsWith(`${base}/`))) return path;
  return `${base}${path}`;
}

export const esc = (text) => String(text)
  .replace(/&/g, '&amp;')
  .replace(/</g, '&lt;')
  .replace(/>/g, '&gt;')
  .replace(/"/g, '&quot;');

export const slugify = (text) => text
  .toLowerCase()
  .replace(/<[^>]+>/g, '')
  .replace(/[`*]/g, '')
  .replace(/&[a-z]+;/g, '')
  .replace(/[^a-z0-9]+/g, '-')
  .replace(/^-|-$/g, '');

export const plain = (md) => md
  .replace(/<\/?kbd>/g, '')
  .replace(/!\[[^\]]*\]\([^)]*\)/g, '')
  .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
  .replace(/[`*]/g, '')
  .replace(/\s+/g, ' ')
  .trim();

export function inline(text, rewrite = (href) => href) {
  const held = [];
  const hold = (html) => `\u0000${held.push(html) - 1}\u0000`;
  text = text.replace(/`([^`]+)`/g, (_, code) => hold(`<code>${esc(code)}</code>`));
  text = text.replace(/<kbd>(.*?)<\/kbd>/g, (_, key) => hold(`<kbd>${esc(key)}</kbd>`));
  text = esc(text);
  text = text.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (_, label, href) => {
    const url = rewrite(href);
    const external = /^https?:/.test(url);
    return hold(`<a href="${esc(url)}"${external ? ' rel="noopener"' : ''}>${label}</a>`);
  });
  text = text.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  text = text.replace(/(^|[\s(])\*([^*\s][^*]*?)\*(?=[\s).,;:!?]|$)/g, '$1<em>$2</em>');
  return text.replace(/\u0000(\d+)\u0000/g, (_, i) => held[i]).replace(/\u0000(\d+)\u0000/g, (_, i) => held[i]);
}

function highlight(code, lang) {
  if (lang === 'json' || lang === 'jsonc') {
    return esc(code).replace(/(&quot;(?:[^&]|&(?!quot;))*?&quot;)(\s*:)?|\b(true|false|null)\b|(-?\b\d+(?:\.\d+)?\b)/g, (match, str, colon, lit, num) =>
      str ? `<span class="${colon ? 'tk-key' : 'tk-str'}">${str}</span>${colon || ''}` : lit ? `<span class="tk-lit">${lit}</span>` : `<span class="tk-num">${num}</span>`);
  }
  if (['sh', 'bash', 'shell', 'console', 'text'].includes(lang)) {
    return code.split('\n').map((line) => {
      if (/^\s*#/.test(line)) return `<span class="tk-comment">${esc(line)}</span>`;
      const command = line.match(/^(\s*(?:\$\s+)?)([\w./-]+)(.*)$/);
      if (!command) return esc(line);
      const rest = esc(command[3]).replace(/(&quot;.*?&quot;|'[^']*')/g, '<span class="tk-str">$1</span>').replace(/(\s)(--?[\w-]+)/g, '$1<span class="tk-flag">$2</span>');
      return `${esc(command[1])}<span class="tk-cmd">${esc(command[2])}</span>${rest}`;
    }).join('\n');
  }
  return esc(code);
}

function listMatch(line) {
  const match = line.match(/^(\s*)([-*]|\d+\.)\s+(.*)$/);
  if (!match) return null;
  return {
    indent: match[1].length,
    ordered: /\d/.test(match[2]),
    start: /\d/.test(match[2]) ? Number.parseInt(match[2], 10) : 1,
    text: match[3],
  };
}

function rewriteHtml(html, rewrite) {
  return html.replace(/(src|href)="([^"]+)"/g, (_, attr, value) => `${attr}="${esc(rewrite(value))}"`);
}

export function render(markdown, rewrite = (href) => href) {
  const lines = markdown.replace(/\r/g, '').split('\n');
  const out = [];
  const toc = [];
  const ids = new Set();
  const uniqueId = (text) => {
    let id = slugify(text) || 'section';
    let n = 2;
    while (ids.has(id)) id = `${slugify(text)}-${n++}`;
    ids.add(id);
    return id;
  };

  function parseList(index) {
    const first = listMatch(lines[index]);
    const indent = first.indent;
    const ordered = first.ordered;
    const items = [];
    while (index < lines.length) {
      if (!lines[index].trim()) {
        let next = index + 1;
        while (next < lines.length && !lines[next].trim()) next += 1;
        const peeked = next < lines.length ? listMatch(lines[next]) : null;
        if (peeked && peeked.indent >= indent) {
          index = next;
          continue;
        }
        break;
      }
      const item = listMatch(lines[index]);
      if (!item || item.indent !== indent || item.ordered !== ordered) break;
      let text = item.text;
      index += 1;
      const nested = [];
      while (index < lines.length) {
        if (!lines[index].trim()) {
          let next = index + 1;
          while (next < lines.length && !lines[next].trim()) next += 1;
          const peeked = next < lines.length ? listMatch(lines[next]) : null;
          if (peeked && peeked.indent > indent) {
            index = next;
            continue;
          }
          break;
        }
        const deeper = listMatch(lines[index]);
        if (deeper && deeper.indent > indent) {
          const child = parseList(index);
          nested.push(child.html);
          index = child.index;
          continue;
        }
        if (lines[index].startsWith(' '.repeat(indent + 2)) && !listMatch(lines[index])) {
          text += ` ${lines[index].trim()}`;
          index += 1;
          continue;
        }
        break;
      }
      items.push(`${inline(text, rewrite)}${nested.join('')}`);
    }
    const tag = ordered ? 'ol' : 'ul';
    const start = ordered && first.start > 1 ? ` start="${first.start}"` : '';
    return { html: `<${tag}${start}>${items.map((item) => `<li>${item}</li>`).join('')}</${tag}>`, index };
  }

  let i = 0;
  const blockStart = (line) => /^(#{1,4} |```|> |\s*[-*] |\s*\d+\. |\||!\[|<)/.test(line);
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim() || /^<!--/.test(line.trim())) {
      i += 1;
      continue;
    }
    let match;
    if ((match = line.match(/^(#{2,4}) (.+)$/))) {
      const level = match[1].length;
      const id = uniqueId(match[2]);
      if (level === 2) toc.push({ id, text: plain(match[2]) });
      out.push(`<h${level} id="${id}"><a class="anchor" href="#${id}" aria-hidden="true" tabindex="-1">#</a>${inline(match[2], rewrite)}</h${level}>`);
      i += 1;
      continue;
    }
    if ((match = line.match(/^```(\S*)\s*(.*)$/))) {
      const lang = match[1];
      const title = match[2];
      const body = [];
      i += 1;
      while (i < lines.length && !lines[i].startsWith('```')) body.push(lines[i++]);
      i += 1;
      const label = title || { sh: 'Terminal', bash: 'Terminal', shell: 'Terminal', json: 'JSON', text: '', ini: 'INI' }[lang] || lang;
      out.push(`<div class="code-block"${lang ? ` data-lang="${esc(lang)}"` : ''}>${label ? `<div class="code-head"><span>${esc(label)}</span></div>` : ''}<button class="code-copy" type="button" aria-label="Copy code"><span class="icon" aria-hidden="true"></span></button><pre><code>${highlight(body.join('\n'), lang)}</code></pre></div>`);
      continue;
    }
    if (line.startsWith('> ')) {
      const body = [];
      while (i < lines.length && lines[i].startsWith('>')) body.push(lines[i++].replace(/^>\s?/, ''));
      out.push(`<blockquote>${render(body.join('\n'), rewrite).html}</blockquote>`);
      continue;
    }
    if ((match = line.match(/^!\[([^\]]*)\]\(([^)\s]+)\)\s*$/))) {
      out.push(`<figure class="doc-shot"><img src="${esc(rewrite(match[2]))}" alt="${esc(match[1])}" loading="lazy" decoding="async">${match[1] ? `<figcaption>${inline(match[1], rewrite)}</figcaption>` : ''}</figure>`);
      i += 1;
      continue;
    }
    if (/^<(figure|img)\b/.test(line.trim())) {
      const block = [line];
      if (!line.includes('</figure>') && line.trim().startsWith('<figure')) {
        i += 1;
        while (i < lines.length && !lines[i].includes('</figure>')) block.push(lines[i++]);
        if (i < lines.length) block.push(lines[i]);
      }
      out.push(rewriteHtml(block.join('\n'), rewrite));
      i += 1;
      continue;
    }
    if (line.startsWith('|')) {
      const rows = [];
      while (i < lines.length && lines[i].startsWith('|')) rows.push(lines[i++]);
      const cells = (row) => row.trim().replace(/^\||\|$/g, '').split(/(?<!\\)\|/).map((cell) => cell.trim().replace(/\\\|/g, '|'));
      if (rows.length < 2) continue;
      const head = cells(rows[0]);
      const align = cells(rows[1]).map((cell) => (cell.startsWith(':') && cell.endsWith(':') ? 'center' : cell.endsWith(':') ? 'right' : ''));
      const cell = (tag, value, column) => `<${tag}${align[column] ? ` style="text-align:${align[column]}"` : ''}>${inline(value, rewrite)}</${tag}>`;
      out.push(`<div class="table-wrap"><table><thead><tr>${head.map((value, column) => cell('th', value, column)).join('')}</tr></thead><tbody>${rows.slice(2).map((row) => `<tr>${cells(row).map((value, column) => cell('td', value, column)).join('')}</tr>`).join('')}</tbody></table></div>`);
      continue;
    }
    if (listMatch(line)) {
      const list = parseList(i);
      out.push(list.html);
      i = list.index;
      continue;
    }
    const para = [];
    while (i < lines.length && lines[i].trim() && !(para.length && blockStart(lines[i]))) para.push(lines[i++].trim());
    out.push(`<p>${inline(para.join(' '), rewrite)}</p>`);
  }
  return { html: out.join('\n'), toc };
}

export function splitPage(markdown, file) {
  const lines = markdown.replace(/\r/g, '').split('\n');
  let index = 0;
  while (index < lines.length && !lines[index].trim()) index += 1;
  const heading = lines[index]?.match(/^# (.+)$/);
  if (!heading) throw new Error(`${file}: missing h1`);
  const title = heading[1].trim();
  const body = lines.slice(index + 1).join('\n').trim();
  const paragraph = [];
  for (const line of body.split('\n')) {
    if (!line.trim()) {
      if (paragraph.length) break;
      continue;
    }
    if (blockStartLine(line)) {
      if (paragraph.length) break;
      continue;
    }
    paragraph.push(line.trim());
  }
  const lead = paragraph.join(' ');
  let description = plain(lead) || title;
  if (description.length > 180) description = `${description.slice(0, 177).replace(/\s+\S*$/, '')}…`;
  return { title, lead: lead || description, description, body: `${body}\n` };
}

function blockStartLine(line) {
  return /^(#{1,4} |```|> |\s*[-*] |\s*\d+\. |\||!\[|<)/.test(line);
}

const NAV = [
  ['/#features', 'Features'],
  ['/docs/playbooks/', 'Guides'],
  ['/docs/', 'Docs'],
];

export function header(active = '') {
  const links = NAV.map(([href, label]) => `<a href="${rootPath(href)}"${label === active ? ' aria-current="page"' : ''}>${label}</a>`).join('');
  return `<header class="site-header">
      <a class="wordmark" href="${rootPath('/')}" aria-label="Taskmaster home"><img class="app-mark" src="${rootPath('/assets/brand/taskmaster-icon.svg')}" width="26" height="26" alt="">Taskmaster</a>
      <nav class="site-nav" aria-label="Main">${links}</nav>
      <div class="header-actions"><a class="github-link" href="${repo}" aria-label="Taskmaster on GitHub" title="Taskmaster on GitHub"><span class="icon" data-icon="github" aria-hidden="true"></span><span class="star-count" aria-hidden="true"></span></a><a class="nav-download" href="${repo}/releases/latest">Download</a></div>
    </header>`;
}

export const footer = `<footer class="site-footer">
      <div class="footer-main">
        <div class="footer-about">
          <a class="footer-brand" href="${rootPath('/')}" aria-label="Taskmaster home"><img class="app-mark" src="${rootPath('/assets/brand/taskmaster-icon.svg')}" width="26" height="26" alt="">Taskmaster</a>
          <p>A native command center for coding agents.</p>
        </div>
        <nav class="footer-columns" aria-label="Resources">
          <div><h3>Product</h3><a href="${repo}/releases/latest">Download</a><a href="${rootPath('/install/')}">Install</a><a href="${rootPath('/releases/')}">Releases</a><a href="${rootPath('/docs/')}">Documentation</a><a href="${rootPath('/docs/playbooks/')}">Playbooks</a></div>
          <div><h3>Project</h3><a href="${repo}">GitHub</a><a href="${devRepo}/introduction.md">Developer docs</a><a href="${rootPath('/docs/roadmap/')}">Roadmap</a><a href="${rootPath('/security/')}">Security</a></div>
          <div><h3>Legal</h3><a href="${repo}/blob/main/LICENSE">Apache 2.0</a><a href="${rootPath('/docs/privacy/')}">Privacy</a><a href="${repo}/blob/main/SECURITY.md">Security policy</a></div>
        </nav>
      </div>
    </footer>`;

export function shell({ title, description, path, body, extraCss = '', scripts = '', sky = false }) {
  const url = abs(path);
  return `<!doctype html>
<html lang="en" data-base="${esc(base)}">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="theme-color" content="#05070c">
  <title>${esc(title)}</title>
  <meta name="description" content="${esc(description)}">
  <link rel="canonical" href="${esc(url)}">
  <meta property="og:site_name" content="Taskmaster">
  <meta property="og:title" content="${esc(title)}">
  <meta property="og:description" content="${esc(description)}">
  <meta property="og:url" content="${esc(url)}">
  <meta property="og:image" content="${esc(abs('/assets/preview.png'))}">
  <meta name="twitter:card" content="summary_large_image">
  <link rel="icon" href="${rootPath('/favicon.svg')}" type="image/svg+xml">
  <link rel="preload" href="${rootPath('/assets/fonts/geist.woff2')}" as="font" type="font/woff2" crossorigin>
  <link rel="stylesheet" href="${rootPath('/style.css')}">
  <link rel="stylesheet" href="${rootPath('/docs.css')}">
  <link rel="stylesheet" href="${rootPath('/guides.css')}">
  <link rel="stylesheet" href="${rootPath('/taskmaster.css')}">
  ${extraCss}
  <script type="module" src="${rootPath('/stats.js')}"></script>
  ${scripts}
</head>
<body>
  ${sky ? '<div class="page-sky" aria-hidden="true"></div>' : ''}
  <a class="skip-link" href="#main">Skip to content</a>
  <div class="site-wrap">
    ${body}
    ${footer}
  </div>
</body>
</html>
`;
}
