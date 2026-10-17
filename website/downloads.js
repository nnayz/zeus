const repository = 'https://github.com/nnayz/taskmaster';
export const latestReleaseAPI = 'https://api.github.com/repos/nnayz/taskmaster/releases/latest';

export function assetUrls(version) {
  const tag = `v${version}`;
  return {
    version,
    tag,
    github: `${repository}/releases/tag/${tag}`,
    notes: `${repository}/releases/tag/${tag}`,
    releases: `${repository}/releases`,
    dmg: `${repository}/releases/download/${tag}/taskmaster-${version}-universal.dmg`,
    zip: `${repository}/releases/download/${tag}/taskmaster-${version}-universal.zip`,
  };
}

// Only the macOS DMG and ZIP from a finished stable release. Drafts, prereleases,
// helpers, and unrelated archives are ignored.
export function releaseDownloads(release) {
  if (!release || release.draft !== false || release.prerelease !== false
      || !/^v?\d+\.\d+\.\d+$/.test(release.tag_name || '') || !Array.isArray(release.assets)) return null;
  const version = release.tag_name.replace(/^v/, '');
  const tag = encodeURIComponent(release.tag_name);
  const urls = assetUrls(version);
  const formats = [
    { name: `taskmaster-${version}-universal.dmg`, label: 'macOS · Universal DMG', primary: true, url: `${repository}/releases/download/${tag}/taskmaster-${version}-universal.dmg` },
    { name: `taskmaster-${version}-universal.zip`, label: 'macOS · Universal ZIP', url: `${repository}/releases/download/${tag}/taskmaster-${version}-universal.zip` },
  ];
  const downloads = formats.flatMap((format) => {
    const asset = release.assets.find((item) => item?.name === format.name
      && item.state === 'uploaded' && Number.isSafeInteger(item.size) && item.size > 0
      && item.browser_download_url === format.url);
    return asset ? [{ ...format }] : [];
  });
  return { version: `v${version}`, notes: `${repository}/releases/tag/${tag}`, downloads, fallback: urls };
}

export async function loadLatestRelease(fetchRelease = globalThis.fetch) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 5000);
  try {
    const response = await fetchRelease(latestReleaseAPI, {
      headers: { Accept: 'application/vnd.github+json' },
      credentials: 'omit',
      signal: controller.signal,
    });
    if (!response.ok) return null;
    return releaseDownloads(await response.json());
  } catch {
    return null;
  } finally {
    clearTimeout(timeout);
  }
}

export async function initDownloads(document, fetchRelease) {
  const primary = document.querySelector('#download-primary');
  if (!primary) return;
  const release = await loadLatestRelease(fetchRelease);
  const mac = release?.downloads.find((asset) => asset.primary);
  if (!mac) return;
  primary.href = mac.url;
  const label = primary.querySelector('.button-label');
  if (label) label.textContent = 'Download for macOS';
  primary.setAttribute('aria-label', `Download Taskmaster ${release.version} for macOS, universal DMG`);
  const version = document.querySelector('#download-version');
  if (version) version.textContent = `${release.version} · macOS 15+`;
  const notes = document.querySelector('#download-notes');
  if (notes) notes.href = release.notes;
  const assets = document.querySelector('#download-assets');
  if (assets) {
    const links = release.downloads.filter((asset) => !asset.primary).map((asset) => {
      const link = document.createElement('a');
      link.href = asset.url;
      link.textContent = asset.label;
      return link;
    });
    assets.replaceChildren(...links);
  }
}

if (typeof document !== 'undefined') void initDownloads(document);
