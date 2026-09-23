import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, basename, dirname } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));

function filesUnder(dir, predicate) {
  if (!existsSync(dir)) throw new Error(`Package directory does not exist: ${dir}`);
  const found = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) found.push(...filesUnder(path, predicate));
    else if (entry.isFile() && predicate(path)) found.push(path);
  }
  return found;
}

export function locateBundledCli(dir, target) {
  const windows = target.includes('windows');
  const executable = windows ? 'BililiveRecorder.Cli.exe' : 'BililiveRecorder.Cli';
  // Match the *target directory* of the complete official CLI resource,
  // not a build input, a similarly named launcher, or a system CLI.
  const candidates = filesUnder(dir, (path) =>
    basename(path) === executable && path.split(/[\\/]/).includes(target)
      && path.split(/[\\/]/).includes('sidecar'));
  if (candidates.length !== 1) {
    throw new Error(`Expected one packaged ${executable} for ${target}; found ${candidates.length}: ${candidates.join(', ')}`);
  }
  return candidates[0];
}

function run(program, args, timeout = 240_000) {
  const result = spawnSync(program, args, { encoding: 'utf8', timeout, maxBuffer: 1024 * 1024 });
  if (result.error || result.status !== 0) {
    throw new Error(`${program} failed (${result.status ?? result.error}): ${result.stderr || result.stdout}`);
  }
  return result.stdout;
}

function onlyPackage(dir, suffix) {
  const packages = filesUnder(dir, (path) => path.endsWith(suffix));
  if (packages.length !== 1) throw new Error(`Expected one ${suffix} package in ${dir}, found ${packages.length}`);
  return packages[0];
}

export function verify(target = process.env.TAURI_TARGET_TRIPLE, { projectRoot = root, runCommand = run } = {}) {
  if (!target) throw new Error('TAURI_TARGET_TRIPLE is required');
  const bundle = resolve(projectRoot, 'src-tauri', 'target', target, 'release', 'bundle');
  const version = JSON.parse(readFileSync(join(projectRoot, 'upstream.json'), 'utf8')).version.replace(/^v/, '');
  // Only the CLI's semantic version is checked. This does NOT verify WebUI,
  // GUI startup, updater, process termination, or recording integrity.
  const temp = mkdtempSync(join(tmpdir(), 'bililive-package-smoke-'));
  try {
    let unpacked;
    if (target.includes('apple-darwin')) {
      const plist = onlyPackage(join(bundle, 'macos'), '.app/Contents/Info.plist');
      unpacked = dirname(dirname(plist)); // the single .app, never its siblings
    } else if (target.includes('linux')) {
      const deb = onlyPackage(join(bundle, 'deb'), '.deb');
      unpacked = join(temp, 'deb');
      runCommand('dpkg-deb', ['--extract', deb, unpacked]);
    } else if (target.includes('windows')) {
      const msi = onlyPackage(join(bundle, 'msi'), '.msi');
      unpacked = join(temp, 'msi');
      runCommand('msiexec.exe', ['/a', msi, '/qn', `TARGETDIR=${unpacked}`, '/L*v', join(temp, 'msiexec.log')]);
    } else {
      throw new Error(`Unsupported target ${target}`);
    }
    const cli = locateBundledCli(unpacked, target);
    const actual = runCommand(cli, ['--version'], 60_000).trim();
    // The official CLI prints e.g. 2.20.0+Branch.tags-v2.20.0.Sha.<hash>.
    // Ignore only SemVer build metadata; the whole X.Y.Z core must match.
    const parsed = /^(\d+\.\d+\.\d+)(?:\+[0-9A-Za-z.-]+)?$/.exec(actual);
    if (!parsed || parsed[1] !== version) throw new Error(`Bundled CLI version mismatch: expected ${version}, got ${actual}`);
    console.log(`Packaged CLI executable smoke succeeded: ${target}, CLI ${actual}`);
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) verify();
