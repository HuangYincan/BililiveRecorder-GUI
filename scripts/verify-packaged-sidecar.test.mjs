import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { locateBundledCli, verify } from './verify-packaged-sidecar.mjs';

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

test('full macOS verification confines the CLI to the .app and compares exact core version', () => {
  const projectRoot = mkdtempSync(join(tmpdir(), 'bililive-mac-package-'));
  const target = 'aarch64-apple-darwin';
  const macos = join(projectRoot, 'src-tauri', 'target', target, 'release', 'bundle', 'macos');
  const app = join(macos, 'Mikufans录播姬.app');
  const resources = join(app, 'Contents', 'Resources', 'sidecar', target);
  const outside = join(macos, 'outside-any-app', 'sidecar', target);
  const cli = join(resources, 'BililiveRecorder.Cli');
  const decoy = join(outside, 'BililiveRecorder.Cli');
  try {
    writeFileSync(join(projectRoot, 'upstream.json'), '{"version":"v2.20.0"}\n');
    mkdirSync(resources, { recursive: true });
    mkdirSync(outside, { recursive: true });
    writeFileSync(join(app, 'Contents', 'Info.plist'), 'plist fixture');
    writeFileSync(decoy, 'outside CLI must never be selected');
    // This exercises verify() itself, not merely locateBundledCli(). The
    // injected command keeps the fixture portable; CI runs the real binary.
    const result = (output) => verify(target, {
      projectRoot,
      runCommand(program, args) {
        assert.equal(program, cli);
        assert.deepEqual(args, ['--version']);
        return output;
      },
    });
    assert.throws(() => result('2.20.0'), /Expected one packaged/);
    writeFileSync(cli, 'fixture CLI');
    assert.doesNotThrow(() => result('2.20.0+Branch.tags-v2.20.0.Sha.abc123\n'));
    for (const version of ['12.20.0', '2.20.1', 'v2.20.0', '2.20.0 extra']) {
      assert.throws(() => result(version), /version mismatch/, version);
    }
  } finally {
    rmSync(projectRoot, { recursive: true, force: true });
  }
});
