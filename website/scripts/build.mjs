import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { buildSite } from './site.mjs';
import { base, repoRoot, root } from './render.mjs';

const dist = resolve(root, 'dist');
await rm(dist, { recursive: true, force: true });
await cp(resolve(root, 'static'), dist, { recursive: true });
await cp(resolve(root, 'assets'), resolve(dist, 'assets'), { recursive: true });
await cp(resolve(repoRoot, 'docs/src/images'), resolve(dist, 'docs/images'), { recursive: true });
for (const file of ['style.css', 'docs.css', 'guides.css', 'taskmaster.css']) {
  let css = await readFile(resolve(root, file), 'utf8');
  if (base) css = css.replaceAll("url('/", `url('${base}/`).replaceAll('url("/', `url("${base}/`);
  await writeFile(resolve(dist, file), css);
}
for (const file of ['docs.js', 'docs-search.js', 'downloads.js', 'stats.js', 'app.js', 'agent-previews.js', 'favicon.svg']) {
  await cp(resolve(root, file), resolve(dist, file));
}
const outputs = await buildSite();
for (const [file, content] of outputs) {
  const dest = resolve(dist, file);
  await mkdir(dirname(dest), { recursive: true });
  await writeFile(dest, content);
}
await writeFile(resolve(dist, '.nojekyll'), '');
console.log(`Built website/dist (${outputs.size} files, base ${base || '/'}).`);
