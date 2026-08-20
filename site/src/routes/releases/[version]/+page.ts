import { error } from '@sveltejs/kit';
import { releaseByVersion, releases } from '$lib/releases';
import type { EntryGenerator, PageLoad } from './$types';

export const entries: EntryGenerator = () =>
  releases.map((release) => ({ version: release.version }));

export const load: PageLoad = ({ params }) => {
  const release = releaseByVersion(params.version);
  if (!release) {
    error(404, 'Unknown release');
  }
  return { release };
};
