import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { locateBundledCli } from './verify-packaged-sidecar.mjs';

test('locates exactly one bundled CLI by target and sidecar resource path', () => {
  const dir = mkdtempSync(join(tmpdir(), 'bililive-package-fixture-'));
  try {
    for (const target of ['aarch64-apple-darwin', 'x86_64-unknown-linux-gnu', 'x86_64-pc-windows-msvc']) {
      const folder = join(dir, target, 'sidecar', target);
      mkdirSync(folder, { recursive: true });
      const executable = join(folder, `BililiveRecorder.Cli${target.includes('windows') ? '.exe' : ''}`);
      writeFileSync(executable, 'stand-in');
      assert.equal(locateBundledCli(join(dir, target), target), executable);
    }
    assert.throws(() => locateBundledCli(join(dir, 'x86_64-unknown-linux-gnu'), 'aarch64-apple-darwin'), /one packaged/);
    assert.throws(() => locateBundledCli(dir, 'not-a-target'), /does not exist|one packaged/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
