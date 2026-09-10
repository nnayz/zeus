import { env } from '$env/dynamic/private';
import { fetchGithubStars } from '$lib/github';

export async function load({ fetch }) {
  return {
    githubStars: await fetchGithubStars(fetch, {
      token: env.GITHUB_TOKEN,
      signal: AbortSignal.timeout(5000)
    })
  };
}
