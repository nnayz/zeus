import { base } from '$app/paths';

export type Release = {
  version: string;
  date: string;
  summary: string;
  unsigned: boolean;
  github: string;
  dmg: string;
  zip: string;
};

export const GITHUB = 'https://github.com/nnayz/zeus';
export const DOCS = `${base}/docs/`;
export const SECURITY_EMAIL = 'hi@nasrul.info';

export const releases: Release[] = [
  {
    version: '0.0.1',
    date: '2026-08-20',
    summary: 'First public cut. Ad-hoc signed, not notarized.',
    unsigned: true,
    github: `${GITHUB}/releases/tag/v0.0.1`,
    dmg: `${GITHUB}/releases/download/v0.0.1/zeus-0.0.1-universal.dmg`,
    zip: `${GITHUB}/releases/download/v0.0.1/zeus-0.0.1-universal.zip`
  }
];

export function latestRelease(): Release {
  return releases[0];
}

export function releaseByVersion(version: string): Release | undefined {
  return releases.find((release) => release.version === version);
}

export function formatDate(iso: string): string {
  const date = new Date(`${iso}T00:00:00Z`);
  return new Intl.DateTimeFormat('en-US', {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
    timeZone: 'UTC'
  }).format(date);
}
