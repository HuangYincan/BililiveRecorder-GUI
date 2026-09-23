import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { execFileSync, spawnSync } from 'node:child_process';

const script = fileURLToPath(new URL('./ensure-release-tag.sh', import.meta.url));
function git(cwd, ...args) {
  return execFileSync('git', args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}
function fixture(run) {
  const dir = mkdtempSync(join(tmpdir(), 'release-tag-fixture-'));
  try {
    const remote = join(dir, 'remote.git');
    const checkout = join(dir, 'checkout');
    git(dir, 'init', '--bare', remote);
    git(dir, 'clone', remote, checkout);
    git(checkout, 'config', 'user.name', 'Fixture');
    git(checkout, 'config', 'user.email', 'fixture@example.invalid');
    writeFileSync(join(checkout, 'package.json'), '{"version":"2.20.1"}\n');
    git(checkout, 'add', 'package.json');
    git(checkout, 'commit', '-m', 'initial release');
    const earlier = git(checkout, 'rev-parse', 'HEAD');
    writeFileSync(join(checkout, 'marker'), 'new build\n');
    git(checkout, 'add', 'marker');
    git(checkout, 'commit', '-m', 'approved build');
    const built = git(checkout, 'rev-parse', 'HEAD');
    git(checkout, 'push', 'origin', 'HEAD:refs/heads/main');
    const execute = (sha = built) => spawnSync('bash', [script], {
      cwd: checkout, encoding: 'utf8', env: { ...process.env, GITHUB_SHA: sha },
    });
    run({ checkout, earlier, built, execute, git });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test('new draft with no Git tag creates one pointing to the approved build', () => fixture(({ checkout, built, execute, git: g }) => {
  const result = execute();
  assert.equal(result.status, 0, result.stderr);
  assert.equal(g(checkout, 'ls-remote', 'origin', 'refs/tags/app-v2.20.1').split('\t')[0], built);
}));
test('existing annotated correct tag is peeled without modification', () => fixture(({ checkout, built, execute, git: g }) => {
  g(checkout, 'tag', '-a', '-m', 'signed separately', 'app-v2.20.1', built);
  g(checkout, 'push', 'origin', 'refs/tags/app-v2.20.1');
  const result = execute();
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /Verified app-v2\.20\.1/);
}));
test('existing tag at a different commit is refused, never overwritten', () => fixture(({ checkout, earlier, execute, git: g }) => {
  g(checkout, 'tag', 'app-v2.20.1', earlier);
  g(checkout, 'push', 'origin', 'refs/tags/app-v2.20.1');
  const result = execute();
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Refusing to publish/);
  assert.equal(g(checkout, 'ls-remote', 'origin', 'refs/tags/app-v2.20.1').split('\t')[0], earlier);
}));
test('run SHA not equal to checkout is refused before tag creation', () => fixture(({ checkout, earlier, execute, git: g }) => {
  const result = execute(earlier);
  assert.notEqual(result.status, 0);
  assert.equal(g(checkout, 'ls-remote', 'origin', 'refs/tags/app-v2.20.1'), '');
}));

test('remote lookup error cannot be mistaken for an absent tag', () => fixture(({ checkout, execute, git: g }) => {
  g(checkout, 'remote', 'set-url', 'origin', join(checkout, 'missing-remote.git'));
  const result = execute();
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Cannot determine whether/);
}));
