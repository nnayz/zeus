import { buildDocs } from './docs.mjs';
import { buildPages, loadReleases } from './pages.mjs';
import { abs, devRepo, header, repo, rootPath, shell } from './render.mjs';

export async function buildSite() {
  const docs = await buildDocs();
  const releases = await loadReleases();
  const pages = await buildPages(releases);
  const outputs = new Map([...docs.outputs, ...pages.outputs]);
  const blurb = `Taskmaster is a native macOS command center for coding agents. User documentation and playbooks are one site under /docs/. Developer documentation stays on GitHub and is not published here.`;
  outputs.set('llms.txt', `# Taskmaster\n\n> ${blurb}\n\nEach user-facing page is also available as Markdown.\n\n${docs.llms}\n\n## Elsewhere\n\n- [Install](${abs('/install/')}): macOS DMG and Gatekeeper exception.\n- [Source code](${repo}): Apache-2.0.\n- [Developer docs](${devRepo}/introduction.md): GitHub only.\n`);
  outputs.set('llms-full.txt', `# Taskmaster user documentation\n\n> ${blurb}\n${docs.full}\n`);
  const urls = [...pages.urls, ...docs.urls, '/docs/dev/'];
  const body = urls.map((url) => `  <url><loc>${abs(url)}</loc></url>`).join('\n');
  outputs.set('sitemap.xml', `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${body}\n</urlset>\n`);
  outputs.set('robots.txt', `User-agent: *\nAllow: /\n\nSitemap: ${abs('/sitemap.xml')}\n`);
  outputs.set('404.html', shell({
    title: 'Not found · Taskmaster',
    description: 'That page is not on the Taskmaster website.',
    path: '/404.html',
    body: `${header('')}
    <main id="main"><header class="guide-heading"><h1>Page not found</h1><p>User docs and playbooks are both under <a href="${rootPath('/docs/')}">/docs/</a>. Developer docs stay on <a href="${devRepo}/introduction.md" rel="noopener">GitHub</a>.</p></header></main>`,
  }).replace('<head>', '<head>\n  <meta name="robots" content="noindex">'));
  return outputs;
}
