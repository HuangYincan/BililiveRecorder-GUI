import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { releaseAssetNames } from './release-asset-names.mjs';

export function verifyUploadedReleaseAssets(release, version, sha) {
  if (release.tagName !== `app-v${version}` || release.targetCommitish !== sha || !release.isDraft) {
    throw new Error('Draft release tag, approved commit or unpublished state differs');
  }
  const expected = releaseAssetNames(version).all;
  const actual = release.assets.map((asset) => asset.name);
  if (new Set(actual).size !== actual.length) throw new Error('Release has colliding asset names');
  const missing = expected.filter((name) => !actual.includes(name));
  const unexpected = actual.filter((name) => !expected.includes(name));
  if (missing.length || unexpected.length) {
    throw new Error(`Release asset identity mismatch: missing=${missing.join(',')}; unexpected=${unexpected.join(',')}`);
  }
  for (const asset of release.assets) {
    if (asset.size <= 0 || !/^sha256:[a-f0-9]{64}$/.test(asset.digest || '')) {
      throw new Error(`Release asset missing size or digest: ${asset.name}`);
    }
  }
  return expected;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [path, version, sha] = process.argv.slice(2);
  if (!path || !version || !sha) throw new Error('Usage: node scripts/verify-uploaded-release-assets.mjs <draft.json> <version> <approved-sha>');
  const result = verifyUploadedReleaseAssets(JSON.parse(readFileSync(path, 'utf8')), version, sha);
  console.log(`Verified ${result.length} exact ASCII draft assets, SHA-256 digests and approved build identity`);
}
