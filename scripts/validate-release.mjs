import { readFileSync } from 'node:fs';
import { basename } from 'node:path';

const [manifestPath, assetsPath] = process.argv.slice(2);
if (!manifestPath || !assetsPath) {
  throw new Error('Usage: node scripts/validate-release.mjs <latest.json> <assets.json>');
}

const expectedVersion = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')).version;
const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
const assets = new Set(
  JSON.parse(readFileSync(assetsPath, 'utf8')).assets.map((asset) => asset.name),
);
const releaseTag = `app-v${expectedVersion}`;
const requiredPlatforms = [
  'darwin-aarch64',
  'darwin-x86_64',
  'linux-x86_64',
  'windows-x86_64',
];
const requiredInstallers = [
  /_aarch64\.dmg$/,
  /_x64\.dmg$/,
  /_x64-setup\.exe$/,
  /_amd64\.deb$/,
  /\.x86_64\.rpm$/,
  /_amd64\.AppImage$/,
];

if (manifest.version !== expectedVersion) {
  throw new Error(`Updater version ${manifest.version} does not match package ${expectedVersion}`);
}

for (const platform of requiredPlatforms) {
  const update = manifest.platforms?.[platform];
  if (!update?.url || !update?.signature) {
    throw new Error(`Updater manifest is missing ${platform}`);
  }

  const url = new URL(update.url);
  if (url.hostname !== 'github.com' || !url.pathname.includes(`/releases/download/${releaseTag}/`)) {
    throw new Error(`${platform} points outside ${releaseTag}: ${update.url}`);
  }

  const assetName = basename(decodeURIComponent(url.pathname));
  if (!assets.has(assetName) || !assets.has(`${assetName}.sig`)) {
    throw new Error(`${platform} updater asset or signature is missing: ${assetName}`);
  }

  const signature = Buffer.from(update.signature, 'base64').toString('utf8');
  if (!signature.startsWith('untrusted comment: signature from tauri secret key')) {
    throw new Error(`${platform} has an invalid embedded updater signature`);
  }
}

for (const pattern of requiredInstallers) {
  if (![...assets].some((asset) => pattern.test(asset))) {
    throw new Error(`Release is missing installer matching ${pattern}`);
  }
}

console.log(`Validated ${releaseTag}: ${requiredPlatforms.join(', ')}`);
