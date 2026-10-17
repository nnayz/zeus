const repo = 'https://api.github.com/repos/nnayz/taskmaster';

const cached = (key) => {
  try {
    const hit = JSON.parse(sessionStorage.getItem(key) || 'null');
    return hit && Date.now() - hit.at < 36e5 ? hit.value : null;
  } catch { return null; }
};
const remember = (key, value) => { try { sessionStorage.setItem(key, JSON.stringify({ at: Date.now(), value })); } catch {} };

export async function starCount() {
  const hit = cached('taskmaster.stars');
  if (hit !== null) return hit;
  const response = await fetch(repo, { headers: { Accept: 'application/vnd.github+json' }, credentials: 'omit' });
  if (!response.ok) throw new Error(String(response.status));
  const stars = (await response.json()).stargazers_count;
  if (!Number.isSafeInteger(stars)) throw new Error('stars');
  remember('taskmaster.stars', stars);
  return stars;
}

export const compact = (n) => n >= 1000 ? `${(n / 1000).toFixed(n >= 10000 ? 0 : 1).replace(/\.0$/, '')}k` : String(n);

if (typeof document !== 'undefined') {
  starCount().then((stars) => {
    for (const el of document.querySelectorAll('.star-count')) {
      el.textContent = compact(stars);
      el.classList.add('loaded');
    }
    for (const link of document.querySelectorAll('.github-link')) link.setAttribute('aria-label', `Taskmaster on GitHub, ${stars} stars`);
  }).catch(() => {});
}
