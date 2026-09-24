import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { releaseAssetNames } from './release-asset-names.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const version = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')).version;
const names = releaseAssetNames(version);
const signature = Buffer.from('untrusted comment: signature from tauri secret key\nfixture').toString('base64');

function fixture(action) {
  const dir = mkdtempSync(join(tmpdir(), 'ascii-release-validate-'));
  const signatureDir = join(dir, 'sigs');
  mkdirSync(signatureDir);
  const assets = { assets: names.all.map((name) => ({ name })) };
  const manifest = {
    version,
    platforms: Object.fromEntries(Object.entries(names.updater).map(([platform, name]) =>
      [platform, { url: `https://github.com/HuangYincan/BililiveRecorder-GUI/releases/download/app-v${version}/${name}`, signature }])),
  };
  for (const name of Object.values(names.updater)) writeFileSync(join(signatureDir, `${name}.sig`), signature);
  try {
    action({ assets, manifest, signatureDir, run() {
      const manifestPath = join(dir, 'latest.json');
      const assetsPath = join(dir, 'assets.json');
      writeFileSync(manifestPath, JSON.stringify(manifest));
      writeFileSync(assetsPath, JSON.stringify(assets));
      return spawnSync(process.execPath,
        [join(root, 'scripts/validate-release.mjs'), manifestPath, assetsPath, signatureDir],
        { cwd: root, encoding: 'utf8' });
    } });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test('exact four updater URLs, all installers and real .sig file contents agree', () => {
  fixture(({ run }) => assert.equal(run().status, 0));
});

test('mismatched signature bytes, missing installer and a uniquely wrong updater URL fail', () => {
  fixture(({ signatureDir, run }) => {
    writeFileSync(join(signatureDir, `${names.updater['darwin-aarch64']}.sig`), `${signature}wrong`);
    assert.match(run().stderr, /signature differs/);
  });
  fixture(({ assets, run }) => {
    assets.assets = assets.assets.filter((asset) => asset.name !== names.installers[0]);
    assert.match(run().stderr, /missing exact installer/);
  });
  fixture(({ manifest, run }) => {
    manifest.platforms['darwin-aarch64'].url += '.WRONG';
    assert.match(run().stderr, /updater asset or signature is missing/);
  });
});
