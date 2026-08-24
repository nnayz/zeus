import { base } from '$app/paths';

export type Release = {
  version: string;
  date: string;
  summary: string[];
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
    version: '0.2.0',
    date: '2026-08-24',
    summary: [
      'Terminal interaction now carries complete mouse tracking and input-mode state through local sessions, remote sessions, and reconnects. Full-screen TUIs receive application-cursor keys, bracketed paste, focus events, alternate scrolling, and click, drag, motion, SGR, and UTF-8 mouse reports consistently.',
      'Terminal geometry now settles from the viewport instead of competing paint measurements, eliminating resize ping-pong and the resulting grid flicker.',
      'MCP session creation now honors the configured default agent, and protected working-directory handling is stricter and more predictable.'
    ],
    unsigned: true,
    github: `${GITHUB}/releases/tag/v0.2.0`,
    dmg: `${GITHUB}/releases/download/v0.2.0/zeus-0.2.0-universal.dmg`,
    zip: `${GITHUB}/releases/download/v0.2.0/zeus-0.2.0-universal.zip`
  },
  {
    version: '0.1.0',
    date: '2026-08-21',
    summary: [
      'Zeus’s first public release gives coding agents a calm, native home on macOS. Run Claude Code, Codex, Cursor, Gemini, and other agents side by side, keep concurrent work isolated in Git worktrees, and see at a glance which sessions are working, waiting, or done.',
      'Every session runs in a real PTY managed by the Engine, so you can close the window, reopen the app, or recover from a restart without losing the process behind the conversation.',
      'Local projects and remote SSH hosts follow the same workflow, with a companion CLI for hooks, notifications, and automation. This is an early, ad-hoc-signed build, but the core promise is already here: your agents keep working, their state stays understandable, and you remain in control.'
    ],
    unsigned: true,
    github: `${GITHUB}/releases/tag/v0.1.0`,
    dmg: `${GITHUB}/releases/download/v0.1.0/zeus-0.1.0-universal.dmg`,
    zip: `${GITHUB}/releases/download/v0.1.0/zeus-0.1.0-universal.zip`
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
