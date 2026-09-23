import { readFileSync, writeFileSync } from 'node:fs';

const root = new URL('../', import.meta.url);
const manifestUrl = new URL('upstream.json', root);
const packageUrl = new URL('package.json', root);
const tauriConfigUrl = new URL('src-tauri/tauri.conf.json', root);
const cargoManifestUrl = new URL('src-tauri/Cargo.toml', root);
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
if (!/^\d+\.\d+\.\d+(?:[-+].+)?$/.test(version)) {
  throw new Error(`Unsupported upstream version: ${release.tag_name}`);
}

writeFileSync(manifestUrl, `${JSON.stringify({
  repository: manifest.repository,
  version: release.tag_name,
  publishedAt: release.published_at,
  releaseUrl: release.html_url,
}, null, 2)}\n`);

const packageJson = JSON.parse(readFileSync(packageUrl, 'utf8'));
packageJson.version = version;
writeFileSync(packageUrl, `${JSON.stringify(packageJson, null, 2)}\n`);

const tauriConfig = JSON.parse(readFileSync(tauriConfigUrl, 'utf8'));
tauriConfig.version = version;
writeFileSync(tauriConfigUrl, `${JSON.stringify(tauriConfig, null, 2)}\n`);

const cargoManifest = readFileSync(cargoManifestUrl, 'utf8');
writeFileSync(cargoManifestUrl, cargoManifest.replace(/^version = "[^"]+"/m, `version = "${version}"`));

console.log(`Synced ${manifest.version} -> ${release.tag_name}`);
