import { releaseAssetNames } from './release-asset-names.mjs';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const [assetsPath, signaturesDirectory, outputPath] = process.argv.slice(2);
if (!assetsPath || !signaturesDirectory || !outputPath) {
  throw new Error(
    'Usage: node scripts/generate-release-manifest.mjs <assets.json> <signatures-directory> <output.json>',
  );
}

const packageJson = JSON.parse(
  readFileSync(new URL('../package.json', import.meta.url), 'utf8'),
);
const repository = process.env.GITHUB_REPOSITORY;
if (!repository) {
  throw new Error('GITHUB_REPOSITORY is required');
}

const releaseTag = `app-v${packageJson.version}`;
const assets = new Set(
  JSON.parse(readFileSync(assetsPath, 'utf8')).assets.map((asset) => asset.name),
);
// This is a distribution filename, not the Unicode application display name.
// The action's pinned ASCII upload pattern and the draft-asset gate use the
// same explicit platform/version mapping; an impostor cannot be selected.
const expected = releaseAssetNames(packageJson.version).updater;

const platforms = Object.fromEntries(
  Object.entries(expected).map(([platform, assetName]) => {
    if (!assets.has(assetName)) {
      throw new Error(`Expected exact ${assetName} ${platform} updater, found 0`);
    }
    const signatureName = `${assetName}.sig`;
    if (!assets.has(assetName) || !assets.has(signatureName)) {
      throw new Error(`${platform} updater asset or signature is missing: ${assetName}`);
    }

    const signature = readFileSync(join(signaturesDirectory, signatureName), 'utf8').trim();
    const decodedSignature = Buffer.from(signature, 'base64').toString('utf8');
    if (!decodedSignature.startsWith('untrusted comment: signature from tauri secret key')) {
      throw new Error(`${platform} has an invalid updater signature: ${signatureName}`);
    }

    return [
      platform,
      {
        signature,
        url: `https://github.com/${repository}/releases/download/${releaseTag}/${encodeURIComponent(assetName)}`,
      },
    ];
  }),
);

writeFileSync(
  outputPath,
  `${JSON.stringify(
    {
      version: packageJson.version,
      notes:
        'This release bundles the matching official BililiveRecorder CLI and its embedded upstream WebUI.\n\nUpstream release metadata is recorded in `upstream.json`.',
      pub_date: new Date().toISOString(),
      platforms,
    },
    null,
    2,
  )}\n`,
);

console.log(`Generated ${releaseTag} updater manifest: ${Object.keys(platforms).join(', ')}`);
