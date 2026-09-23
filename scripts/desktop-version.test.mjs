import test from 'node:test';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';
import { compareVersions, nextDesktopVersion, parseVersion, writeDesktopVersion } from './desktop-version.mjs';

test('a shell hotfix increases the updater version without changing upstream', () => {
  assert.equal(nextDesktopVersion('2.20.0', '2.20.0'), '2.20.1');
});
test('an upstream version cannot reuse a shell-hotfix version', () => {
  assert.equal(nextDesktopVersion('2.20.1', '2.20.1'), '2.20.2');
  assert.equal(nextDesktopVersion('2.21.3', '2.21.0'), '2.21.4');
});
test('a newer upstream can set the next desktop version', () => {
  assert.equal(nextDesktopVersion('2.20.1', '2.21.0'), '2.21.0');
  assert.equal(compareVersions('3.0.0', '2.99.99'), 1);
});
test('nonstable desktop versions are rejected before writing', () => {
  assert.throws(() => parseVersion('2.20.0-rc.1'), /stable/);
});

test('a shell bump updates all five manifests but keeps the upstream CLI pinned', () => {
  const dir = mkdtempSync(join(tmpdir(), 'desktop-version-'));
  try {
    mkdirSync(join(dir, 'src-tauri'));
    const fixtures = {
      'package.json': '{"version":"2.20.0"}\n',
      'package-lock.json': '{"version":"2.20.0","packages":{"":{"version":"2.20.0"}}}\n',
      'src-tauri/tauri.conf.json': '{"version":"2.20.0"}\n',
      'src-tauri/Cargo.toml': '[package]\nname = "bililive-recorder-gui"\nversion = "2.20.0"\n',
      'src-tauri/Cargo.lock': '[[package]]\nname = "bililive-recorder-gui"\nversion = "2.20.0"\n',
      'upstream.json': '{"version":"v2.20.0"}\n',
    };
    for (const [path, content] of Object.entries(fixtures)) writeFileSync(join(dir, path), content);
    assert.equal(writeDesktopVersion('2.20.1', pathToFileURL(`${dir}/`)), '2.20.1');
    for (const path of Object.keys(fixtures).filter((name) => name !== 'upstream.json')) {
      assert.match(readFileSync(join(dir, path), 'utf8'), /2\.20\.1/);
    }
    assert.equal(readFileSync(join(dir, 'upstream.json'), 'utf8'), fixtures['upstream.json']);
    assert.throws(() => writeDesktopVersion('2.20.1', pathToFileURL(`${dir}/`)), /must increase/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
