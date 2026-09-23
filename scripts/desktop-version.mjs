import { readFileSync, writeFileSync } from 'node:fs';

const root = new URL('../', import.meta.url);

export function parseVersion(version) {
  if (!/^\d+\.\d+\.\d+$/.test(version)) {
    throw new Error(`Expected a stable three-part desktop version, got ${version}`);
  }
  return version.split('.').map(Number);
}

export function compareVersions(a, b) {
  const left = parseVersion(a);
  const right = parseVersion(b);
  for (let i = 0; i < 3; i++) {
    if (left[i] !== right[i]) return left[i] > right[i] ? 1 : -1;
  }
  return 0;
}

export function nextDesktopVersion(current, upstream) {
  // A new upstream build also needs a *new* signed desktop bundle. The two
  // version series are independent: a previous shell hotfix may already use
  // the upstream's version number (or a higher one).
  if (compareVersions(upstream, current) > 0) return upstream;
  const [major, minor, patch] = parseVersion(current);
  return `${major}.${minor}.${patch + 1}`;
}

export function writeDesktopVersion(next, directory = root) {
  parseVersion(next);
  const packageUrl = new URL('package.json', directory);
  const lockUrl = new URL('package-lock.json', directory);
  const configUrl = new URL('src-tauri/tauri.conf.json', directory);
  const cargoUrl = new URL('src-tauri/Cargo.toml', directory);
  const cargoLockUrl = new URL('src-tauri/Cargo.lock', directory);
  const pkg = JSON.parse(readFileSync(packageUrl, 'utf8'));
  const lock = JSON.parse(readFileSync(lockUrl, 'utf8'));
  const config = JSON.parse(readFileSync(configUrl, 'utf8'));
  const cargo = readFileSync(cargoUrl, 'utf8');
  const cargoLock = readFileSync(cargoLockUrl, 'utf8');
  const current = pkg.version;
  const [cargoEntry, ...cargoRest] = cargoLock.split(/(?=\[\[package\]\]\n)/);
  const entries = [cargoEntry, ...cargoRest];
  const index = entries.findIndex((entry) => /^\[\[package\]\]\nname = "bililive-recorder-gui"\n/m.test(entry));
  if (index < 0) throw new Error('Missing application entry in Cargo.lock');
  const originalEntry = entries[index];
  const match = originalEntry.match(/^version = "([^"]+)"$/m);
  if (!match || [lock.version, lock.packages?.['']?.version, config.version, cargo.match(/^version = "([^"]+)"$/m)?.[1], match[1]]
    .some((version) => version !== current)) {
    throw new Error('Desktop versions are out of sync; refusing a partial bump');
  }
  if (compareVersions(next, current) <= 0) throw new Error(`Desktop version must increase: ${current} -> ${next}`);

  pkg.version = next;
  lock.version = next;
  lock.packages[''].version = next;
  config.version = next;
  entries[index] = originalEntry.replace(/^version = "[^"]+"$/m, `version = "${next}"`);
  writeFileSync(packageUrl, `${JSON.stringify(pkg, null, 2)}\n`);
  writeFileSync(lockUrl, `${JSON.stringify(lock, null, 2)}\n`);
  writeFileSync(configUrl, `${JSON.stringify(config, null, 2)}\n`);
  writeFileSync(cargoUrl, cargo.replace(/^version = "[^"]+"$/m, `version = "${next}"`));
  writeFileSync(cargoLockUrl, entries.join(''));
  return next;
}
