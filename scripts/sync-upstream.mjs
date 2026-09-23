import { readFileSync, writeFileSync } from 'node:fs';
import { nextDesktopVersion, writeDesktopVersion } from './desktop-version.mjs';

const root = new URL('../', import.meta.url);
const manifestUrl = new URL('upstream.json', root);
const manifest = JSON.parse(readFileSync(manifestUrl, 'utf8'));

const token = process.env.GITHUB_TOKEN || process.env.GH_TOKEN;
const headers = {
    Accept: 'application/vnd.github+json',
    'User-Agent': 'BililiveRecorder-GUI upstream sync',
};
if (token) headers.Authorization = `Bearer ${token}`;

const response = await fetch(`https://api.github.com/repos/${manifest.repository}/releases/latest`, {
  headers,
});
if (!response.ok) throw new Error(`GitHub API failed: ${response.status} ${response.statusText}`);

const release = await response.json();
const requiredAssets = [
  'BililiveRecorder-CLI-osx-arm64.zip',
  'BililiveRecorder-CLI-osx-x64.zip',
  'BililiveRecorder-CLI-linux-x64.zip',
  'BililiveRecorder-CLI-linux-arm64.zip',
  'BililiveRecorder-CLI-win-x64.zip',
];
const availableAssets = new Set(release.assets.map((asset) => asset.name));
const missingAssets = requiredAssets.filter((asset) => !availableAssets.has(asset));
if (missingAssets.length) throw new Error(`Latest release lacks required assets: ${missingAssets.join(', ')}`);

if (release.tag_name === manifest.version) {
  console.log(`Already synced to ${manifest.version}`);
  process.exit(0);
}

const version = release.tag_name.replace(/^v/, '');
if (!/^\d+\.\d+\.\d+$/.test(version)) {
  throw new Error(`Unsupported upstream version: ${release.tag_name}`);
}

const current = JSON.parse(readFileSync(new URL('package.json', root), 'utf8')).version;
const desktopVersion = nextDesktopVersion(current, version);
// Bump first: a mismatch must not leave upstream.json ahead of the desktop.
writeDesktopVersion(desktopVersion);
writeFileSync(manifestUrl, `${JSON.stringify({
  repository: manifest.repository,
  version: release.tag_name,
  publishedAt: release.published_at,
  releaseUrl: release.html_url,
}, null, 2)}\n`);
console.log(`Synced ${manifest.version} -> ${release.tag_name}; desktop ${current} -> ${desktopVersion}`);
