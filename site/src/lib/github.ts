import { GITHUB_OWNER, GITHUB_REPO } from '$lib/releases';

const REPO_API = `https://api.github.com/repos/${GITHUB_OWNER}/${GITHUB_REPO}`;

export async function fetchGithubStars(
  fetchFn: typeof fetch = fetch,
  options: { token?: string; signal?: AbortSignal; cache?: RequestCache } = {}
): Promise<number | null> {
  const headers = new Headers({
    Accept: 'application/vnd.github+json',
    'X-GitHub-Api-Version': '2022-11-28'
  });
  if (options.token) {
    headers.set('Authorization', `Bearer ${options.token}`);
  }

  try {
    const response = await fetchFn(REPO_API, {
      headers,
      signal: options.signal,
      cache: options.cache
    });
    if (!response.ok) return null;
    const body: unknown = await response.json();
    if (
      typeof body !== 'object' ||
      body === null ||
      !('stargazers_count' in body) ||
      typeof body.stargazers_count !== 'number' ||
      !Number.isFinite(body.stargazers_count) ||
      body.stargazers_count < 0
    ) {
      return null;
    }
    return body.stargazers_count;
  } catch {
    return null;
  }
}

export function formatStarCount(count: number): string {
  return new Intl.NumberFormat('en', {
    notation: count >= 1000 ? 'compact' : 'standard',
    maximumFractionDigits: 1
  }).format(count);
}
