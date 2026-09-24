import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const version = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')).version;
const productName = JSON.parse(readFileSync(join(root, 'src-tauri/tauri.conf.json'), 'utf8')).productName;
const names = [
  `${productName}_aarch64.app.tar.gz`,
  `${productName}_x64.app.tar.gz`,
  `${productName}_${version}_amd64.AppImage`,
  `${productName}_${version}_x64-setup.exe`,
];
const sig = Buffer.from('untrusted comment: signature from tauri secret key\nfixture').toString('base64');

function fixture(action) {
  const dir = mkdtempSync(join(tmpdir(), 'brand-release-manifest-'));
  const signatureDir = join(dir, 'signatures');
  mkdirSync(signatureDir);
  const assetsPath = join(dir, 'assets.json');
  const outputPath = join(dir, 'latest.json');
  const assets = names.flatMap((name) => [{ name }, { name: `${name}.sig` }]);
  for (const name of names) writeFileSync(join(signatureDir, `${name}.sig`), sig);
  try {
    action({ assets, assetsPath, signatureDir, outputPath, run() {
      writeFileSync(assetsPath, JSON.stringify({ assets }));
      const result = spawnSync(process.execPath, [join(root, 'scripts/generate-release-manifest.mjs'), assetsPath, signatureDir, outputPath],
        { cwd: root, env: { ...process.env, GITHUB_REPOSITORY: 'HuangYincan/BililiveRecorder-GUI' }, encoding: 'utf8' });
      if (result.status !== 0) throw new Error(result.stderr || result.error?.message);
      return result.stdout;
    } });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test('future updater references exactly the branded Unicode assets and signatures', () => {
  fixture(({ outputPath, run }) => {
    run();
    const manifest = JSON.parse(readFileSync(outputPath, 'utf8'));
    assert.equal(manifest.version, version);
    for (const entry of Object.values(manifest.platforms)) {
      assert.equal(entry.signature, sig);
      assert.equal(entry.url.includes(encodeURIComponent(productName)), true);
      assert.equal(entry.url.includes('/app-v' + version + '/'), true);
    }
  });
});

test('old-product assets and a unique false-name asset fail closed', () => {
  fixture(({ assets, run }) => {
    assets[0].name = 'BililiveRecorder.GUI_aarch64.app.tar.gz';
    assert.throws(run, /Expected exactly one/);
  });
  // This must be a fully well-formed wrong candidate (including its .sig
  // asset and local signature); otherwise the old, loose startsWith matcher
  // would reject for a missing signature and this test would pass accidentally.
  for (const [index, wrongName] of [
    [0, `${productName}.WRONG_aarch64.app.tar.gz`],
    [4, `${productName}.WRONG_${version}_amd64.AppImage`],
  ]) {
    fixture(({ assets, signatureDir, run }) => {
      assets[index].name = wrongName;
      assets[index + 1].name = `${wrongName}.sig`;
      writeFileSync(join(signatureDir, `${wrongName}.sig`), sig);
      assert.throws(run, /Expected exactly one/);
    });
  }
});
