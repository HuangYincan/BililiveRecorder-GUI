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
    action({ assets, assetsPath, outputPath, run() {
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

test('old-product updater assets and ambiguous matching assets fail closed', () => {
  fixture(({ assets, run }) => {
    assets[0].name = 'BililiveRecorder.GUI_aarch64.app.tar.gz';
    assert.throws(run, /Expected exactly one/);
  });
  fixture(({ assets, run }) => {
    assets.push({ name: `${productName}.duplicate_${version}_amd64.AppImage` });
    assert.throws(run, /Expected exactly one/);
  });
});
